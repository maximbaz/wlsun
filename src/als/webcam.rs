use crate::frame::compute_perceived_lightness_percent;
use itertools::Itertools;
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Duration;
use v4l::buffer::Type;
use v4l::io::mmap::Stream;
use v4l::io::traits::CaptureStream;
use v4l::video::Capture;
use v4l::{Device, FourCC};

const DEFAULT_LUX: u64 = 100;
const WAITING_SLEEP_MS: u64 = 2000;
const MIN_WAITING_SLEEP_MS: u64 = 1000;

pub struct Webcam {
    webcam_tx: Sender<u64>,
    video: usize,
    sleep_ms: u64,
}

impl Webcam {
    pub fn new(webcam_tx: Sender<u64>, video: usize, sleep_ms: Option<u64>) -> Self {
        Self {
            webcam_tx,
            video,
            sleep_ms: sleep_ms.filter(|&s| s >= MIN_WAITING_SLEEP_MS).unwrap_or(WAITING_SLEEP_MS),
        }
    }

    pub fn run(&mut self) {
        loop {
            self.step();
        }
    }

    fn step(&mut self) {
        if let Ok((rgbs, pixels)) = self.frame() {
            let lux = compute_perceived_lightness_percent(&rgbs, false, pixels) as u64;

            self.webcam_tx
                .send(lux)
                .expect("Unable to send new webcam lux value, channel is dead");
        };

        thread::sleep(Duration::from_millis(self.sleep_ms));
    }

    fn frame(&mut self) -> Result<(Vec<u8>, usize), Box<dyn Error>> {
        let (device, pixels) = Self::setup(self.video)?;
        let mut stream = Stream::new(&device, Type::VideoCapture)?;
        let (rgbs, _) = stream.next()?;

        Ok((rgbs.to_vec(), pixels))
    }

    fn setup(video: usize) -> Result<(Device, usize), Box<dyn Error>> {
        let device = Device::new(video)?;
        let mut format = device.format()?;
        format.fourcc = FourCC::new(b"RGB3");
        let (width, height) = device
            .enum_framesizes(format.fourcc)?
            .into_iter()
            .flat_map(|f| {
                f.size
                    .to_discrete()
                    .into_iter()
                    .map(|d| (d.width, d.height))
                    .collect_vec()
            })
            .min_by(|&(w1, h1), &(w2, h2)| h1.cmp(&h2).then(w1.cmp(&w2)))
            .ok_or("Unable to find minimum resolution")?;

        format.height = height;
        format.width = width;
        device.set_format(&format)?;

        Ok((device, width as usize * height as usize))
    }
}

pub struct Als {
    webcam_rx: Receiver<u64>,
    thresholds: HashMap<u64, String>,
    lux: RefCell<u64>,
}

impl Als {
    pub fn new(webcam_rx: Receiver<u64>, thresholds: HashMap<u64, String>) -> Self {
        Self {
            webcam_rx,
            thresholds,
            lux: RefCell::new(DEFAULT_LUX),
        }
    }

    fn get_raw(&self) -> Result<u64, Box<dyn Error>> {
        let new_value = self
            .webcam_rx
            .try_iter()
            .last()
            .unwrap_or(*self.lux.borrow());
        *self.lux.borrow_mut() = new_value;
        Ok(new_value)
    }
}

impl super::Als for Als {
    fn get(&self) -> Result<String, Box<dyn Error>> {
        let raw = self.get_raw()?;
        let profile = super::find_profile(raw, &self.thresholds);

        log::trace!("ALS (webcam): {} ({})", profile, raw);
        Ok(profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn setup() -> (Als, Sender<u64>) {
        let (webcam_tx, webcam_rx) = mpsc::channel();
        let als = Als::new(webcam_rx, HashMap::default());
        (als, webcam_tx)
    }

    #[test]
    fn test_sleep_ms_is_custom_value_when_present_in_config() -> Result<(), Box<dyn Error>> {
        let (webcam_tx, _) = mpsc::channel();
        let webcam = Webcam::new(webcam_tx, 0, Some(10000));
        assert_eq!(10000, webcam.sleep_ms);
        Ok(())
    }

    #[test]
    fn test_sleep_ms_is_default_value_when_not_present_in_config() -> Result<(), Box<dyn Error>> {
        let (webcam_tx, _) = mpsc::channel();
        let webcam = Webcam::new(webcam_tx, 0, None);
        assert_eq!(WAITING_SLEEP_MS, webcam.sleep_ms);
        Ok(())
    }

    #[test]
    fn test_sleep_ms_is_default_value_when_invalid_in_config() -> Result<(), Box<dyn Error>> {
        let (webcam_tx, _) = mpsc::channel();
        let webcam = Webcam::new(webcam_tx, 0, Some(MIN_WAITING_SLEEP_MS - 1));
        assert_eq!(WAITING_SLEEP_MS, webcam.sleep_ms);
        Ok(())
    }

    #[test]
    fn test_get_raw_returns_default_value_when_no_data_from_webcam() -> Result<(), Box<dyn Error>> {
        let (als, _) = setup();

        assert_eq!(DEFAULT_LUX, als.get_raw()?);
        Ok(())
    }

    #[test]
    fn test_get_raw_returns_value_from_webcam() -> Result<(), Box<dyn Error>> {
        let (als, webcam_tx) = setup();

        webcam_tx.send(42)?;

        assert_eq!(42, als.get_raw()?);
        Ok(())
    }

    #[test]
    fn test_get_raw_returns_most_recent_value_from_webcam() -> Result<(), Box<dyn Error>> {
        let (als, webcam_tx) = setup();

        webcam_tx.send(42)?;
        webcam_tx.send(43)?;
        webcam_tx.send(44)?;

        assert_eq!(44, als.get_raw()?);
        Ok(())
    }

    #[test]
    fn test_get_raw_returns_last_known_value_from_webcam_when_no_new_data(
    ) -> Result<(), Box<dyn Error>> {
        let (als, webcam_tx) = setup();

        webcam_tx.send(42)?;
        webcam_tx.send(43)?;

        assert_eq!(43, als.get_raw()?);
        assert_eq!(43, als.get_raw()?);
        assert_eq!(43, als.get_raw()?);
        Ok(())
    }
}
