#![allow(dead_code)]
#![allow(unused_imports)]

mod file;
mod state;

pub use file::*;
pub use state::*;

#[repr(u8)]
pub enum SamplePolyAreas {
    Ground,
    Water,
    Road,
    Door,
    Grass,
    Jump,
}

#[repr(u16)]
pub enum SamplePolyFlags {
    Walk = 0x01,
    Swim = 0x02,
    Door = 0x04,
    Jump = 0x08,
    Disabled = 0x10,
    All = 0xff,
}
