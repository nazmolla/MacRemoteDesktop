//! (vendored, divergence 27) A generic seam for extra dynamic virtual channels:
//! the application installs `DvcFactory`s and each connection gets one
//! processor per factory. With none installed, the channel set is unchanged.

use core::any::Any;

use ironrdp_core::{AsAny, Encode, EncodeResult, WriteCursor};
use ironrdp_dvc::{DvcEncode, DvcMessage, DvcProcessor, DvcServerProcessor};
use ironrdp_pdu::PduResult;

pub trait DvcFactory: Send {
    /// A fresh processor for one connection.
    fn build(&self) -> Box<dyn DvcProcessor>;
}

/// Lets a boxed processor be attached like a concrete one.
pub(crate) struct BoxedDvc(pub(crate) Box<dyn DvcProcessor>);

impl AsAny for BoxedDvc {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl DvcProcessor for BoxedDvc {
    fn channel_name(&self) -> &str {
        self.0.channel_name()
    }
    fn start(&mut self, channel_id: u32) -> PduResult<Vec<DvcMessage>> {
        self.0.start(channel_id)
    }
    fn process(&mut self, channel_id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        self.0.process(channel_id, payload)
    }
    fn close(&mut self, channel_id: u32) {
        self.0.close(channel_id);
    }
}

impl DvcServerProcessor for BoxedDvc {}

/// Already-encoded message bytes, sent as one DVC message.
pub struct RawDvcMessage(pub Vec<u8>);

impl RawDvcMessage {
    #[allow(clippy::new_ret_no_self, reason = "returns the boxed wire form")]
    pub fn new(bytes: Vec<u8>) -> DvcMessage {
        Box::new(Self(bytes))
    }
}

impl Encode for RawDvcMessage {
    fn encode(&self, dst: &mut WriteCursor<'_>) -> EncodeResult<()> {
        ironrdp_core::ensure_size!(in: dst, size: self.size());
        dst.write_slice(&self.0);
        Ok(())
    }
    fn name(&self) -> &'static str {
        "RAW_DVC_MESSAGE"
    }
    fn size(&self) -> usize {
        self.0.len()
    }
}

impl DvcEncode for RawDvcMessage {}
