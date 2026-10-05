use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Mono frames downmixed per `push_slice`. The stack buffer is 2 KiB.
const CHUNK: usize = 512;

pub struct Capture {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub audio_rx: AudioRx,
    /// Kept alive. Dropping the stream stops capture.
    pub stream: Stream,
}

pub struct AudioRx {
    cons: ringbuf::HeapCons<f32>,
    dropped: Arc<AtomicU64>,
    logged: u64,
    last_log: Instant,
}

impl AudioRx {
    /// Wrap a ring consumer and the drop counter its producer feeds.
    /// Capture uses it; tests feed synthetic PCM through it.
    pub fn new(cons: ringbuf::HeapCons<f32>, dropped: Arc<AtomicU64>) -> Self {
        Self {
            cons,
            dropped,
            logged: 0,
            last_log: Instant::now(),
        }
    }

    /// Samples the callback failed to push because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Where the pipeline reads mono samples from. Tests feed synthetic PCM.
pub trait AudioSource {
    /// Non-blocking: append whatever is ready to `out`, possibly nothing.
    fn drain(&mut self, out: &mut Vec<f32>);
}

impl AudioSource for AudioRx {
    /// Also logs ring overruns, at most once per second.
    fn drain(&mut self, out: &mut Vec<f32>) {
        out.extend(self.cons.pop_iter());
        let dropped = self.dropped();
        if dropped > self.logged && self.last_log.elapsed() >= Duration::from_secs(1) {
            tracing::warn!(dropped, new = dropped - self.logged, "audio ring overrun");
            self.logged = dropped;
            self.last_log = Instant::now();
        }
    }
}

/// Average each interleaved frame into `out`. Returns mono samples written.
fn downmix<T: Copy>(data: &[T], ch: usize, to_f32: fn(T) -> f32, out: &mut [f32]) -> usize {
    let mut n = 0;
    for (slot, frame) in out.iter_mut().zip(data.chunks(ch)) {
        let sum: f32 = frame.iter().copied().map(to_f32).sum();
        *slot = sum / ch as f32;
        n += 1;
    }
    n
}

/// Downmix and bulk-push. No allocation, no lock. Overflow adds to `dropped`.
fn push_mono<T: Copy>(
    prod: &mut impl Producer<Item = f32>,
    dropped: &AtomicU64,
    data: &[T],
    ch: usize,
    to_f32: fn(T) -> f32,
) {
    let mut buf = [0.0_f32; CHUNK];
    for block in data.chunks(ch * CHUNK) {
        let n = downmix(block, ch, to_f32, &mut buf);
        let pushed = prod.push_slice(&buf[..n]);
        if pushed < n {
            dropped.fetch_add((n - pushed) as u64, Ordering::Relaxed);
        }
    }
}

pub fn start_loopback() -> Result<Capture> {
    let host = cpal::default_host();

    // Loopback: open the *output* device as an input stream.
    // macOS 14.6+ CoreAudio and Windows WASAPI accept this.
    // Linux: select the PipeWire/Pulse ".monitor" source instead, or pass
    // a device name via SUBS_DEVICE.
    let device = match std::env::var("SUBS_DEVICE") {
        Ok(name) => host
            .input_devices()?
            .find(|d| d.name().ok().as_deref() == Some(name.as_str()))
            .ok_or_else(|| anyhow!("no input device named {name}"))?,
        Err(_) => host
            .default_output_device()
            .or_else(|| host.default_input_device())
            .ok_or_else(|| anyhow!("no audio device"))?,
    };
    let device_name = device.name().unwrap_or_else(|_| "unknown".into());

    let config = device
        .default_output_config()
        .or_else(|_| device.default_input_config())?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();

    let rb = HeapRb::<f32>::new(sample_rate as usize * 8);
    let (prod, cons) = rb.split();
    let dropped = Arc::new(AtomicU64::new(0));

    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(
            &device,
            &config.into(),
            prod,
            dropped.clone(),
            channels,
            |s| s,
        )?,
        SampleFormat::I16 => build::<i16>(
            &device,
            &config.into(),
            prod,
            dropped.clone(),
            channels,
            |s| s as f32 / i16::MAX as f32,
        )?,
        SampleFormat::U16 => build::<u16>(
            &device,
            &config.into(),
            prod,
            dropped.clone(),
            channels,
            |s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0,
        )?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    stream.play()?;

    Ok(Capture {
        device_name,
        sample_rate,
        channels,
        audio_rx: AudioRx::new(cons, dropped),
        stream,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut prod: ringbuf::HeapProd<f32>,
    dropped: Arc<AtomicU64>,
    channels: u16,
    to_f32: fn(T) -> f32,
) -> Result<Stream>
where
    T: cpal::SizedSample + 'static,
{
    let ch = channels as usize;
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| push_mono(&mut prod, &dropped, data, ch, to_f32),
        |err| tracing::error!(?err, "audio stream error"),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rx(cap: usize) -> (ringbuf::HeapProd<f32>, AudioRx, Arc<AtomicU64>) {
        let (prod, cons) = HeapRb::<f32>::new(cap).split();
        let dropped = Arc::new(AtomicU64::new(0));
        (prod, AudioRx::new(cons, dropped.clone()), dropped)
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = [0.0; 4];
        let n = downmix(&[1.0_f32, 0.0, 0.5, 0.5, -1.0, -1.0], 2, |s| s, &mut out);
        assert_eq!(n, 3);
        assert_eq!(&out[..3], &[0.5, 0.5, -1.0]);

        let n = downmix(
            &[i16::MAX, i16::MAX],
            2,
            |s| s as f32 / i16::MAX as f32,
            &mut out,
        );
        assert_eq!((n, out[0]), (1, 1.0));
    }

    #[test]
    fn full_ring_counts_drops() {
        let (mut prod, mut rx, dropped) = rx(4);
        push_mono(
            &mut prod,
            &dropped,
            &[1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0],
            1,
            |s| s,
        );
        assert_eq!(rx.dropped(), 2);
        let mut out = Vec::new();
        rx.drain(&mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn bulk_push_spans_chunks() {
        let (mut prod, mut rx, dropped) = rx(2048);
        let data = vec![0.25_f32; 1500 * 2];
        push_mono(&mut prod, &dropped, &data, 2, |s| s);
        assert_eq!(rx.dropped(), 0);
        let mut out = Vec::new();
        rx.drain(&mut out);
        assert_eq!(out.len(), 1500);
        assert!(out.iter().all(|&s| s == 0.25));
    }
}
