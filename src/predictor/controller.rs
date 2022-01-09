use crate::als::Als;
use crate::predictor::data::{Data, Entry};
use crate::predictor::kalman::Kalman;
use itertools::Itertools;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

const INITIAL_BRIGHTNESS_TIMEOUT_SECS: u64 = 2;
const PENDING_COOLDOWN_RESET: u8 = 15;

pub struct Controller {
    prediction_tx: Sender<u64>,
    user_rx: Receiver<u64>,
    als: Box<dyn Als>,
    kalman: Kalman,
    pending_cooldown: u8,
    pending: Option<Entry>,
    data: Data,
    stateful: bool,
    initial_brightness: Option<u64>,
}

impl Controller {
    pub fn new(
        prediction_tx: Sender<u64>,
        user_rx: Receiver<u64>,
        als: Box<dyn Als>,
        stateful: bool,
    ) -> Self {
        let data = if stateful {
            Data::load().unwrap_or_default()
        } else {
            Data::default()
        };

        // Brightness controller is expected to send the initial value on this channel asap
        let initial_brightness = user_rx
            .recv_timeout(Duration::from_secs(INITIAL_BRIGHTNESS_TIMEOUT_SECS))
            .expect("Did not receive initial brightness value in time");

        // If there are no learned entries yet, we will use this as the first data point,
        // assuming that user is happy with the current brightness settings
        let initial_brightness = if data.entries.is_empty() {
            Some(initial_brightness)
        } else {
            None
        };

        Self {
            prediction_tx,
            user_rx,
            als,
            kalman: Kalman::new(1.0, 20.0, 10.0),
            pending_cooldown: 0,
            pending: None,
            data,
            stateful,
            initial_brightness,
        }
    }

    pub fn adjust(&mut self, luma: Option<u8>) {
        let lux = self
            .kalman
            .process(self.als.get().expect("Unable to get ALS value"));

        if self.kalman.initialized() {
            self.process(lux, luma);
        }
    }

    fn process(&mut self, lux: u64, luma: Option<u8>) {
        let initial_brightness = self.initial_brightness.take();
        let user_changed_brightness = self.user_rx.try_iter().last().or(initial_brightness);

        if let Some(brightness) = user_changed_brightness {
            self.pending = match &self.pending {
                // First time we notice user adjusting brightness, freeze lux and luma...
                None => Some(Entry::new(lux, luma, brightness)),
                // ... but as user keeps changing brightness,
                // allow some time for them to reach the desired brightness level for the pending lux and luma
                Some(Entry { lux, luma, .. }) => Some(Entry::new(*lux, *luma, brightness)),
            };
            // Every time user changed brightness, reset the cooldown period
            self.pending_cooldown = PENDING_COOLDOWN_RESET;
        } else if self.pending_cooldown > 0 {
            self.pending_cooldown -= 1;
        } else if self.pending.is_some() {
            self.learn();
        } else {
            self.predict(lux, luma);
        }
    }

    fn learn(&mut self) {
        let pending = self.pending.take().expect("No pending entry to learn");

        self.data.entries.retain(|entry| {
            let darker_env_darker_screen = entry.lux < pending.lux && entry.luma < pending.luma;

            let darker_env_same_screen = entry.lux < pending.lux
                && entry.luma == pending.luma
                && entry.brightness <= pending.brightness;

            let darker_env_brighter_screen = entry.lux < pending.lux
                && entry.luma > pending.luma
                && entry.brightness <= pending.brightness;

            let same_env_darker_screen = entry.lux == pending.lux
                && entry.luma < pending.luma
                && entry.brightness >= pending.brightness;

            let same_env_brighter_screen = entry.lux == pending.lux
                && entry.luma > pending.luma
                && entry.brightness <= pending.brightness;

            let brighter_env_darker_screen = entry.lux > pending.lux
                && entry.luma < pending.luma
                && entry.brightness >= pending.brightness;

            let brighter_env_same_screen = entry.lux > pending.lux
                && entry.luma == pending.luma
                && entry.brightness >= pending.brightness;

            let brighter_env_brighter_screen = entry.lux > pending.lux && entry.luma > pending.luma;

            darker_env_darker_screen
                || darker_env_same_screen
                || darker_env_brighter_screen
                || same_env_darker_screen
                || same_env_brighter_screen
                || brighter_env_darker_screen
                || brighter_env_same_screen
                || brighter_env_brighter_screen
        });

        self.data.entries.push(pending);

        if self.stateful {
            self.data.save().expect("Unable to save data");
        }
    }

    fn predict(&mut self, lux: u64, luma: Option<u8>) {
        if self.data.entries.is_empty() {
            return;
        }

        let points = self
            .data
            .entries
            .iter()
            .map(|entry| {
                let p1 = lux as f64 - entry.lux as f64;
                let p2 = luma.unwrap_or(0) as f64 - entry.luma.unwrap_or(0) as f64;
                let distance = (p1.powf(2.0) + p2.powf(2.0)).sqrt();
                (entry.brightness as f64, distance)
            })
            .collect_vec();

        let points = points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let other_distances: f64 = points[0..i]
                    .iter()
                    .chain(&points[i + 1..])
                    .map(|p| p.1)
                    .product();
                (p.0, p.1, other_distances)
            })
            .collect_vec();

        let distance_denominator: f64 = points
            .iter()
            .map(|p| p.1)
            .combinations(points.len() - 1)
            .map(|c| c.iter().product::<f64>())
            .sum();

        let prediction: f64 = points
            .iter()
            .map(|p| p.0 * p.2 / distance_denominator)
            .sum();

        self.prediction_tx
            .send(prediction as u64)
            .expect("Unable to send predicted brightness value, channel is dead");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::als::MockAls;
    use itertools::iproduct;
    use std::collections::HashSet;
    use std::error::Error;
    use std::sync::mpsc;

    fn setup() -> Result<(Controller, Sender<u64>, Receiver<u64>), Box<dyn Error>> {
        let (user_tx, user_rx) = mpsc::channel();
        let (prediction_tx, prediction_rx) = mpsc::channel();
        user_tx.send(0)?;
        let controller = Controller::new(prediction_tx, user_rx, Box::new(MockAls::new()), false);
        Ok((controller, user_tx, prediction_rx))
    }

    #[test]
    fn test_process_first_user_change() -> Result<(), Box<dyn Error>> {
        let (mut controller, user_tx, _) = setup()?;

        // User changes brightness to value 33 for a given lux and luma
        user_tx.send(33)?;
        controller.process(12345, Some(66));

        assert_eq!(Some(Entry::new(12345, Some(66), 33)), controller.pending);
        assert_eq!(PENDING_COOLDOWN_RESET, controller.pending_cooldown);

        Ok(())
    }

    #[test]
    fn test_process_several_continuous_user_changes() -> Result<(), Box<dyn Error>> {
        let (mut controller, user_tx, _) = setup()?;

        // User initiates brightness change for a given lux and luma to value 33...
        user_tx.send(33)?;
        controller.process(12345, Some(66));
        // then quickly continues increasing it to 34 (while lux and luma might already be different)...
        user_tx.send(34)?;
        controller.process(23456, Some(36));
        // and even faster to 36 (which is the indended brightness value they wish to learn for the initial lux and luma)
        user_tx.send(35)?;
        user_tx.send(36)?;
        controller.process(100, Some(16));

        assert_eq!(Some(Entry::new(12345, Some(66), 36)), controller.pending);
        assert_eq!(PENDING_COOLDOWN_RESET, controller.pending_cooldown);

        Ok(())
    }

    #[test]
    fn test_process_learns_user_change_after_cooldown() -> Result<(), Box<dyn Error>> {
        let (mut controller, user_tx, _) = setup()?;

        // User changes brightness to a desired value
        user_tx.send(33)?;
        controller.process(12345, Some(66));
        user_tx.send(33)?;
        controller.process(23456, Some(36));
        user_tx.send(35)?;
        controller.process(100, Some(16));

        for i in 1..=PENDING_COOLDOWN_RESET {
            // User doesn't change brightness anymore, so even if lux or luma change, we are in cooldown period
            controller.process(100 + i as u64, Some(i));
            assert_eq!(PENDING_COOLDOWN_RESET - i, controller.pending_cooldown);
            assert_eq!(Some(Entry::new(12345, Some(66), 35)), controller.pending);
        }

        // One final process will trigger the learning
        controller.process(200, Some(17));

        assert_eq!(None, controller.pending);
        assert_eq!(0, controller.pending_cooldown);
        assert_eq!(
            vec![Entry::new(12345, Some(66), 35)],
            controller.data.entries
        );

        Ok(())
    }

    // If user configured brightness value in certain conditions (amount of light around, screen contents),
    // how changes in environment or screen contents can affect the desired brightness level:
    //
    // |                 | darker env      | same env         | brighter env     |
    // | darker screen   | any             | same or brighter | same or brighter |
    // | same screen     | same or dimmer  | only same        | same or brighter |
    // | brighter screen | same or dimmer  | same or dimmer   | any              |

    #[test]
    fn test_learn_data_cleanup() -> Result<(), Box<dyn Error>> {
        let (mut controller, _, _) = setup()?;

        let pending = Entry::new(10, Some(20), 30);

        let all_combinations: HashSet<_> = iproduct!(-1i32..=1, -1i32..=1, -1i32..=1)
            .map(|(i, j, k)| Entry::new((10 + i) as u64, Some((20 + j) as u8), (30 + k) as u64))
            .collect();

        let to_be_deleted: HashSet<_> = vec![
            // darker env same screen
            Entry::new(9, Some(20), 31),
            // darker env brighter screen
            Entry::new(9, Some(21), 31),
            // same env darker screen
            Entry::new(10, Some(19), 29),
            // same env same screen
            Entry::new(10, Some(20), 29),
            Entry::new(10, Some(20), 31),
            // same env brighter screen
            Entry::new(10, Some(21), 31),
            // brighter env darker screen
            Entry::new(11, Some(19), 29),
            // brighter env same screen
            Entry::new(11, Some(20), 29),
        ]
        .into_iter()
        .collect();

        controller.data.entries = all_combinations.iter().cloned().collect_vec();
        controller.pending = Some(pending);

        controller.learn();

        let to_remain: HashSet<_> = all_combinations.difference(&to_be_deleted).collect();
        let remained = controller.data.entries.iter().collect();

        assert_eq!(
            Vec::<&&Entry>::new(),
            to_remain.difference(&remained).collect_vec(),
            "unexpected entries were removed"
        );

        assert_eq!(
            Vec::<&&Entry>::new(),
            remained.difference(&to_remain).collect_vec(),
            "some entries were not removed"
        );

        assert_eq!(
            to_remain.len(),
            controller.data.entries.len(),
            "duplicate entries remained"
        );

        Ok(())
    }

    #[test]
    fn test_predict_no_data_points() -> Result<(), Box<dyn Error>> {
        let (mut controller, _, prediction_rx) = setup()?;
        controller.data.entries = vec![];

        // predict() should not be called with no data, but just in case confirm we don't panic
        controller.predict(10, Some(20));

        assert_eq!(true, prediction_rx.try_recv().is_err());

        Ok(())
    }

    #[test]
    fn test_predict_one_data_point() -> Result<(), Box<dyn Error>> {
        let (mut controller, _, prediction_rx) = setup()?;
        controller.data.entries = vec![Entry::new(5, Some(10), 15)];

        controller.predict(10, Some(20));

        assert_eq!(15, prediction_rx.try_recv()?);
        Ok(())
    }

    #[test]
    fn test_predict_known_conditions() -> Result<(), Box<dyn Error>> {
        let (mut controller, _, prediction_rx) = setup()?;
        controller.data.entries = vec![Entry::new(5, Some(10), 15), Entry::new(10, Some(20), 30)];

        controller.predict(10, Some(20));

        assert_eq!(30, prediction_rx.try_recv()?);
        Ok(())
    }

    #[test]
    fn test_predict_approximate() -> Result<(), Box<dyn Error>> {
        let (mut controller, _, prediction_rx) = setup()?;
        controller.data.entries = vec![
            Entry::new(5, Some(10), 15),
            Entry::new(10, Some(20), 30),
            Entry::new(100, Some(100), 100),
        ];

        // Approximated using weighted distance to all known points:
        // dist1 = sqrt((x1 - x2)^2 + (y1 - y2)^2)
        // weight1 = (1/dist1) / (1/dist1 + 1/dist2 + 1/dist3)
        // prediction = weight1*brightness1 + weight2*brightness2 + weight3*brightness
        controller.predict(50, Some(50));

        assert_eq!(44, prediction_rx.try_recv()?);
        Ok(())
    }
}
