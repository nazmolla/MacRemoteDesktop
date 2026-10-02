//! Extra monitors (Phase 4a): each non-primary monitor gets its own EGFX
//! surface, mapped at the monitor's place in the client's output, with its own
//! VideoToolbox encoder and ship thread. The primary monitor keeps the main
//! pipeline in `mod.rs`.

use super::*;

pub(super) struct Lane {
    encoder: Encoder,
}

impl Gfx {
    /// Encode one captured frame of extra monitor `index` (0-based among the
    /// extras), shown at `origin` in the client's output. The lane is created on
    /// first use, once the primary surface exists. Returns whether it was sent.
    pub fn submit_monitor(
        &self,
        index: usize,
        origin: (u32, u32),
        bgra: &[u8],
        stride: usize,
        (w, h): (u16, u16),
    ) -> Result<bool> {
        let (server_handle, missing) = {
            let guard = lock_ctx(&self.ctx);
            match guard.as_ref() {
                Some(ctx) if ctx.is_ready && ctx.surface_id.is_some() => (
                    ctx.server_handle.clone(),
                    ctx.lanes.get(index).is_none_or(Option::is_none),
                ),
                _ => return Ok(false),
            }
        };
        if missing {
            self.create_lane(index, origin, (w, h), &server_handle)?;
        }
        let mut guard = lock_ctx(&self.ctx);
        let Some(lane) = guard
            .as_mut()
            .and_then(|ctx| ctx.lanes.get_mut(index))
            .and_then(Option::as_mut)
        else {
            return Ok(false);
        };
        lane.encoder.encode_bgra(bgra, stride, false)?;
        Ok(true)
    }

    fn create_lane(
        &self,
        index: usize,
        origin: (u32, u32),
        (w, h): (u16, u16),
        server_handle: &GfxServerHandle,
    ) -> Result<()> {
        // Server first, then ctx (see "Lock order").
        let (surface_id, messages, channel) = {
            let mut server = lock_server(server_handle);
            let surface_id = server
                .create_surface_with_format(w, h, PixelFormat::XRgb)
                .ok_or_else(|| anyhow!("EGFX: could not create a surface for monitor {index}"))?;
            server.map_surface_to_output(surface_id, origin.0, origin.1);
            let channel = server
                .channel_id()
                .ok_or_else(|| anyhow!("EGFX: channel_id not assigned"))?;
            (surface_id, server.drain_output(), channel)
        };
        self.send_dvc(channel, messages)?;

        let bitrate = lock_ctx(&self.ctx)
            .as_ref()
            .map_or(4_000_000, |ctx| ctx.adaptive_target_bps);
        let mut encoder = Encoder::new(w, h, self.fps, bitrate, self.keyframe_secs)?;
        let rx = encoder
            .take_receiver()
            .ok_or_else(|| anyhow!("EGFX: lane encoder receiver already taken"))?;
        let gfx = self.clone();
        std::thread::Builder::new()
            .name(format!("egfx-lane-{index}"))
            .spawn(move || {
                let epoch = Instant::now();
                // Ends when the lane's encoder is dropped with its connection.
                while let Ok(frame) = rx.recv() {
                    if let Err(e) = gfx.ship_lane(surface_id, (w, h), &frame, epoch) {
                        warn!(error = ?e, index, "EGFX: monitor lane ship failed");
                    }
                }
            })
            .map_err(|e| anyhow!("spawning the monitor lane thread: {e}"))?;

        let mut guard = lock_ctx(&self.ctx);
        let ctx = guard
            .as_mut()
            .ok_or_else(|| anyhow!("EGFX: connection ended while creating a monitor lane"))?;
        if ctx.lanes.len() <= index {
            ctx.lanes.resize_with(index + 1, || None);
        }
        ctx.lanes[index] = Some(Lane { encoder });
        info!(index, surface_id, w, h, ?origin, "EGFX: monitor lane created");
        Ok(())
    }

    fn ship_lane(
        &self,
        surface_id: u16,
        (w, h): (u16, u16),
        frame: &EncodedFrame,
        epoch: Instant,
    ) -> Result<()> {
        let server_handle = match lock_ctx(&self.ctx).as_ref() {
            Some(ctx) => ctx.server_handle.clone(),
            None => return Ok(()),
        };
        let payload = self.frame_payload(frame);
        let ts_ms = u32::try_from(epoch.elapsed().as_millis() % u128::from(u32::MAX)).unwrap_or(0);
        let (messages, channel) = {
            let mut server = lock_server(&server_handle);
            server.send_avc420_frame(surface_id, &payload, &avc_regions(None, w, h), ts_ms);
            let channel = server
                .channel_id()
                .ok_or_else(|| anyhow!("EGFX: channel_id not assigned"))?;
            (server.drain_output(), channel)
        };
        self.send_dvc(channel, messages)
    }

    fn send_dvc(&self, channel: u32, messages: Vec<ironrdp_dvc::DvcMessage>) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let svc = encode_dvc_messages(channel, messages, ChannelFlags::SHOW_PROTOCOL)
            .map_err(|e| anyhow!("encode_dvc_messages failed: {e}"))?;
        let sender = self
            .sender
            .lock_or_recover()
            .clone()
            .ok_or_else(|| anyhow!("EGFX: server-event sender not set"))?;
        sender
            .send(ServerEvent::Egfx(EgfxServerMessage::SendMessages { messages: svc }))
            .map_err(|_| anyhow!("EGFX: ServerEvent send failed (event loop closed)"))
    }
}
