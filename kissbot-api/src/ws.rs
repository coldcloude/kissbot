use crate::error::Result;

pub struct RequestResponse<R> {
    pub sn: u32,
    pub payload: R,
}

pub const OFFSET_TYPE: usize = 0;
pub const LEN_TYPE: usize = 4;

pub fn parse_bin_type(data: &[u8]) -> Result<u32> {
    let type_bin: [u8; 4] = data[OFFSET_TYPE..OFFSET_TYPE + LEN_TYPE].try_into()?;
    Ok(u32::from_be_bytes(type_bin))
}
