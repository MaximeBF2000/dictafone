use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};
use mp3lame_encoder::{max_required_buffer_size, Bitrate, Builder, FlushNoGap, MonoPcm, Quality};
use tauri::{AppHandle, Emitter};

enum Msg {
    Samples(Vec<f32>),
    Stop,
    Cancel,
}

/// A recording in progress. Audio is captured on a dedicated thread (cpal streams
/// are not `Send`) and encoded to MP3 on the fly.
pub struct Recording {
    tx: Sender<Msg>,
    handle: JoinHandle<Result<Option<PathBuf>, String>>,
}

impl Recording {
    pub fn start(app: AppHandle, output: PathBuf) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel::<Msg>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let sample_tx = tx.clone();

        let handle = thread::spawn(move || run(app, output, sample_tx, rx, ready_tx));

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx, handle }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Recorder thread exited unexpectedly".into()),
        }
    }

    /// Stops recording and writes the MP3. Returns the saved file path.
    pub fn stop(self) -> Result<PathBuf, String> {
        let _ = self.tx.send(Msg::Stop);
        match self.handle.join() {
            Ok(Ok(Some(path))) => Ok(path),
            Ok(Ok(None)) => Err("Recording was cancelled".into()),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("Recorder thread panicked".into()),
        }
    }

    /// Stops recording and discards everything.
    pub fn cancel(self) {
        let _ = self.tx.send(Msg::Cancel);
        let _ = self.handle.join();
    }
}

fn run(
    app: AppHandle,
    output: PathBuf,
    sample_tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ready_tx: Sender<Result<(), String>>,
) -> Result<Option<PathBuf>, String> {
    let setup = || -> Result<(cpal::Stream, u32), String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("No microphone found")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();

        let stream = match format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, sample_tx),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, sample_tx),
            SampleFormat::I32 => build_stream::<i32>(&device, &config, channels, sample_tx),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, channels, sample_tx),
            other => return Err(format!("Unsupported sample format: {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, sample_rate))
    };

    let (stream, sample_rate) = match setup() {
        Ok(v) => {
            let _ = ready_tx.send(Ok(()));
            v
        }
        Err(e) => {
            let _ = ready_tx.send(Err(e.clone()));
            return Err(e);
        }
    };

    let mut encoder = {
        let mut b = Builder::new().ok_or("Failed to create MP3 encoder")?;
        b.set_num_channels(1).map_err(|e| e.to_string())?;
        b.set_sample_rate(sample_rate).map_err(|e| e.to_string())?;
        b.set_brate(Bitrate::Kbps128).map_err(|e| e.to_string())?;
        b.set_quality(Quality::Good).map_err(|e| e.to_string())?;
        b.build().map_err(|e| e.to_string())?
    };
    let mut mp3: Vec<u8> = Vec::new();

    // Level metering for the UI waveform (~30 updates/sec).
    let mut peak = 0f32;
    let mut last_emit = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Samples(chunk)) => {
                let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
                peak = peak.max(rms);
                if last_emit.elapsed() >= Duration::from_millis(33) {
                    let _ = app.emit("level", peak);
                    peak = 0.0;
                    last_emit = Instant::now();
                }

                mp3.reserve(max_required_buffer_size(chunk.len()));
                encoder
                    .encode_to_vec(MonoPcm(chunk.as_slice()), &mut mp3)
                    .map_err(|e| format!("MP3 encoding failed: {e:?}"))?;
            }
            Ok(Msg::Stop) => break,
            Ok(Msg::Cancel) => return Ok(None),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(stream);
    // Drain anything captured between the stop request and the stream shutdown.
    while let Ok(Msg::Samples(chunk)) = rx.try_recv() {
        mp3.reserve(max_required_buffer_size(chunk.len()));
        let _ = encoder.encode_to_vec(MonoPcm(chunk.as_slice()), &mut mp3);
    }

    mp3.reserve(7200);
    encoder
        .flush_to_vec::<FlushNoGap>(&mut mp3)
        .map_err(|e| format!("MP3 flush failed: {e:?}"))?;

    std::fs::write(&output, &mp3).map_err(|e| format!("Could not save file: {e}"))?;
    Ok(Some(output))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: Sender<Msg>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                // Downmix to mono f32.
                let mono: Vec<f32> = data
                    .chunks(channels)
                    .map(|frame| {
                        frame.iter().map(|&s| <f32 as cpal::FromSample<T>>::from_sample_(s)).sum::<f32>()
                            / channels as f32
                    })
                    .collect();
                let _ = tx.send(Msg::Samples(mono));
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| e.to_string())
}
