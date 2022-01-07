use mockall::*;
use std::error::Error;

pub mod iio;
pub mod none;
pub mod time;
pub mod webcam;

#[automock]
pub trait Als {
    fn get(&mut self) -> Result<u64, Box<dyn Error>>;
}

#[allow(clippy::ptr_arg)]
pub fn smoothen(raw_lux: u64, thresholds: &Vec<u64>) -> u64 {
    thresholds
        .iter()
        .enumerate()
        .find(|(_, &threshold)| raw_lux < threshold)
        .map(|(i, _)| i as u64)
        .unwrap_or(thresholds.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smoothen() {
        assert_eq!(0, smoothen(123, &vec![]));
        assert_eq!(0, smoothen(23, &vec![100, 200]));
        assert_eq!(1, smoothen(123, &vec![100, 200]));
        assert_eq!(2, smoothen(223, &vec![100, 200]));
    }
}
