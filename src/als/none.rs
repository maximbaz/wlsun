use std::error::Error;

#[derive(Default)]
pub struct Als {}

impl super::Als for Als {
    fn get(&mut self) -> Result<u64, Box<dyn Error>> {
        Ok(0)
    }
}
