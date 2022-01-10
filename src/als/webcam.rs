use crate::als::smoothen;
use crate::frame::compute_perceived_lightness_percent;
use crate::predictor::kalman::Kalman;
use std::cell::RefCell;
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

pub struct Webcam {
    kalman: Kalman,
    webcam_tx: Sender<u64>,
    video: usize,
}

impl Webcam {
    pub fn new(webcam_tx: Sender<u64>, video: usize) -> Self {
        Self {
            kalman: Kalman::new(1.0, 20.0, 10.0),
            webcam_tx,
            video,
        }
    }

    pub fn run(&mut self) {
        loop {
            self.step();
        }
    }

    fn step(&mut self) {
        if let Ok((rgbs, pixels)) = self.frame() {
            let lux_raw = compute_perceived_lightness_percent(&rgbs, false, pixels) as u64;
            let lux = self.kalman.process(lux_raw);

            self.webcam_tx
                .send(lux)
                .expect("Unable to send new webcam lux value, channel is dead");
        };

        thread::sleep(Duration::from_millis(WAITING_SLEEP_MS));
    }

    fn frame(&mut self) -> Result<(Vec<u8>, usize), Box<dyn Error>> {
        let dev = Device::new(self.video)?;

        let mut fmt = dev.format()?;
        fmt.fourcc = FourCC::new(b"RGB3");
        dev.set_format(&fmt)?;

        let mut stream = Stream::new(&dev, Type::VideoCapture)?;
        let (rgbs, _) = stream.next()?;

        Ok((rgbs.to_vec(), fmt.height as usize * fmt.width as usize))
    }
}

pub struct Als {
    webcam_rx: Receiver<u64>,
    thresholds: Vec<u64>,
    lux: RefCell<u64>,
}

impl Als {
    pub fn new(webcam_rx: Receiver<u64>, thresholds: Vec<u64>) -> Self {
        Self {
            webcam_rx,
            thresholds,
            lux: RefCell::new(DEFAULT_LUX),
        }
    }
}

impl super::Als for Als {
    fn get_raw(&self) -> Result<u64, Box<dyn Error>> {
        let new_value = self
            .webcam_rx
            .try_iter()
            .last()
            .unwrap_or(*self.lux.borrow());
        *self.lux.borrow_mut() = new_value;
        Ok(new_value)
    }

    fn smoothen(&self, raw: u64) -> u64 {
        smoothen(raw, &self.thresholds)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Als as AlsTrait;
    use super::*;
    use std::sync::mpsc;

    fn setup() -> (Als, Sender<u64>) {
        let (webcam_tx, webcam_rx) = mpsc::channel();
        let als = Als::new(webcam_rx, vec![]);
        (als, webcam_tx)
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
