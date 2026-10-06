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

/// Scale is 2^15: `i16::MIN` is -1, `i16::MAX` is one LSB under 1.
fn i16_to_f32(s: i16) -> f32 {
    s as f32 / 32_768.0
}

/// Origin is 32768. Dividing by `u16::MAX` puts silence half an LSB above 0.
fn u16_to_f32(s: u16) -> f32 {
    (s as f32 - 32_768.0) / 32_768.0
}

/// Scale is 2^31. `i32::MAX` is not exact in f32, so it lands on 1.
fn i32_to_f32(s: i32) -> f32 {
    s as f32 / 2_147_483_648.0
}

fn f64_to_f32(s: f64) -> f32 {
    s as f32
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

/// Exact-name match over input devices, then output devices. On macOS the
/// loopback device is an output device. The error lists what is available.
fn find_device(host: &cpal::Host, name: &str) -> Result<cpal::Device> {
    let devices: Vec<_> = host
        .input_devices()
        .into_iter()
        .flatten()
        .chain(host.output_devices().into_iter().flatten())
        .collect();
    let names: Vec<String> = devices
        .iter()
        .map(|d| d.name().unwrap_or_default())
        .collect();
    match pick_name(&names, name) {
        Some(i) => Ok(devices.into_iter().nth(i).expect("index from names")),
        None => Err(anyhow!("no device named {name:?}; available: {names:?}")),
    }
}

/// Index of the first exact name match. Inputs precede outputs in `names`,
/// so a name shared by both resolves to the input.
fn pick_name(names: &[String], want: &str) -> Option<usize> {
    names.iter().position(|n| n == want)
}

/// Print every input and output device: kind, default marker, name, rate, channels.
pub fn list_devices() -> Result<()> {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.name().ok());
    let default_out = host.default_output_device().and_then(|d| d.name().ok());
    let line = |kind: &str,
                default: &Option<String>,
                d: &cpal::Device,
                cfg: Option<cpal::SupportedStreamConfig>| {
        let name = d.name().unwrap_or_else(|_| "unknown".into());
        let star = if default.as_deref() == Some(name.as_str()) {
            '*'
        } else {
            ' '
        };
        match cfg {
            Some(c) => println!(
                "{kind} {star} {name}\t{} Hz\t{} ch",
                c.sample_rate().0,
                c.channels()
            ),
            None => println!("{kind} {star} {name}\t? Hz\t? ch"),
        }
    };
    for d in host.input_devices()? {
        line("in ", &default_in, &d, d.default_input_config().ok());
    }
    for d in host.output_devices()? {
        line("out", &default_out, &d, d.default_output_config().ok());
    }
    Ok(())
}

pub fn start_loopback() -> Result<Capture> {
    let host = cpal::default_host();

    // Loopback: open the *output* device as an input stream.
    // macOS 14.6+ CoreAudio and Windows WASAPI accept this.
    // Linux: select the PipeWire/Pulse ".monitor" source instead, or pass
    // a device name via SUBS_DEVICE. See `--list-devices`.
    let device = match std::env::var("SUBS_DEVICE") {
        Ok(name) => find_device(&host, &name)?,
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
            i16_to_f32,
        )?,
        SampleFormat::U16 => build::<u16>(
            &device,
            &config.into(),
            prod,
            dropped.clone(),
            channels,
            u16_to_f32,
        )?,
        SampleFormat::I32 => build::<i32>(
            &device,
            &config.into(),
            prod,
            dropped.clone(),
            channels,
            i32_to_f32,
        )?,
        SampleFormat::F64 => build::<f64>(
            &device,
            &config.into(),
            prod,
            dropped.clone(),
            channels,
            f64_to_f32,
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
    fn pick_name_table() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // inputs then outputs
        let names = n(&["Mic", "BlackHole 2ch", "Speakers", "BlackHole 2ch"]);
        assert_eq!(pick_name(&names, "Mic"), Some(0));
        assert_eq!(pick_name(&names, "Speakers"), Some(2));
        assert_eq!(pick_name(&names, "BlackHole 2ch"), Some(1)); // input wins
        assert_eq!(pick_name(&names, "blackhole 2ch"), None); // exact, case-sensitive
        assert_eq!(pick_name(&names, "Nope"), None);
        assert_eq!(pick_name(&[], "Mic"), None);
    }

    #[test]
    fn downmix_averages_channels() {
        let mut out = [0.0; 4];
        let n = downmix(&[1.0_f32, 0.0, 0.5, 0.5, -1.0, -1.0], 2, |s| s, &mut out);
        assert_eq!(n, 3);
        assert_eq!(&out[..3], &[0.5, 0.5, -1.0]);

        let n = downmix(&[i16::MIN, i16::MIN], 2, i16_to_f32, &mut out);
        assert_eq!((n, out[0]), (1, -1.0));
    }

    #[test]
    fn sample_to_f32_full_scale() {
        assert_eq!(i16_to_f32(i16::MIN), -1.0);
        assert_eq!(i16_to_f32(0), 0.0);
        assert_eq!(i16_to_f32(i16::MAX), 32_767.0 / 32_768.0);

        assert_eq!(u16_to_f32(0), -1.0);
        assert_eq!(u16_to_f32(32_768), 0.0);
        assert_eq!(u16_to_f32(u16::MAX), 32_767.0 / 32_768.0);

        assert_eq!(i32_to_f32(i32::MIN), -1.0);
        assert_eq!(i32_to_f32(0), 0.0);
        assert_eq!(i32_to_f32(i32::MAX), 1.0);

        assert_eq!(f64_to_f32(-1.0), -1.0);
        assert_eq!(f64_to_f32(0.0), 0.0);
        assert_eq!(f64_to_f32(0.5), 0.5);
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
