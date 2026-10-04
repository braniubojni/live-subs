use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use ringbuf::traits::{Producer, Split};
use ringbuf::HeapRb;
use std::sync::{Arc, Mutex};

pub struct Capture {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub audio_rx: AudioRx,
    /// Kept alive. Dropping the stream stops capture.
    _stream: Stream,
}

pub struct AudioRx {
    inner: Arc<Mutex<ringbuf::HeapCons<f32>>>,
}

impl AudioRx {
    /// Non-blocking drain. The audio callback must never wait on this.
    pub fn drain(&self, out: &mut Vec<f32>) {
        let mut cons = self.inner.lock().expect("audio ring poisoned");
        out.extend(cons.pop_iter());
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
    let prod = Arc::new(Mutex::new(prod));

    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config.into(), prod, channels, |s| s)?,
        SampleFormat::I16 => build::<i16>(&device, &config.into(), prod, channels, |s| {
            s as f32 / i16::MAX as f32
        })?,
        SampleFormat::U16 => build::<u16>(&device, &config.into(), prod, channels, |s| {
            (s as f32 / u16::MAX as f32) * 2.0 - 1.0
        })?,
        other => return Err(anyhow!("unsupported sample format {other:?}")),
    };
    stream.play()?;

    Ok(Capture {
        device_name,
        sample_rate,
        channels,
        audio_rx: AudioRx {
            inner: Arc::new(Mutex::new(cons)),
        },
        _stream: stream,
    })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    prod: Arc<Mutex<ringbuf::HeapProd<f32>>>,
    channels: u16,
    to_f32: fn(T) -> f32,
) -> Result<Stream>
where
    T: cpal::SizedSample,
{
    let ch = channels as usize;
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut prod = prod.lock().expect("audio ring poisoned");
            // Downmix to mono in the callback. No allocation, no model calls.
            for frame in data.chunks(ch) {
                let sum: f32 = frame.iter().copied().map(to_f32).sum();
                let _ = prod.try_push(sum / ch as f32);
            }
        },
        |err| tracing::error!(?err, "audio stream error"),
        None,
    )?;
    Ok(stream)
}
