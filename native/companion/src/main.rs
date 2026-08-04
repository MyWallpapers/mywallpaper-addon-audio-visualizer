use std::f32::consts::TAU;
use std::io;
use std::ptr;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use windows::core::GUID;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

mod framing;

use framing::read_json_record;

const PROTOCOL_VERSION: u32 = 5;
const SPECTRUM_BANDS: usize = 64;
const ANALYSIS_SAMPLES: usize = 1024;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 65_534;
const KSDATAFORMAT_SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: GUID =
    GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

#[derive(Deserialize)]
#[serde(tag = "type")]
enum HostFrame {
    #[serde(rename = "init")]
    Init {
        v: u32,
        #[serde(rename = "layerSettings")]
        _layer_settings: Value,
        #[serde(rename = "deviceSettings")]
        device_settings: Value,
    },
    #[serde(rename = "settings")]
    Settings {
        v: u32,
        #[serde(rename = "layerSettings")]
        _layer_settings: Value,
        #[serde(rename = "deviceSettings")]
        device_settings: Value,
    },
    #[serde(rename = "message")]
    Message {
        v: u32,
        #[serde(rename = "surface")]
        _surface: RendererSurface,
        #[serde(rename = "payload")]
        _payload: Value,
    },
    #[serde(rename = "shutdown")]
    Shutdown { v: u32 },
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum CompanionFrame<T: Serialize> {
    #[serde(rename = "ready")]
    Ready { v: u32 },
    #[serde(rename = "message")]
    Message {
        v: u32,
        target: MessageTarget,
        payload: T,
    },
    #[serde(rename = "error")]
    Error {
        v: u32,
        message: String,
        code: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RendererSurface {
    Interface,
    Wallpaper,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
enum MessageTarget {
    Broadcast,
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum Payload {
    #[serde(rename = "audio.frame")]
    Frame(AudioFrame),
    #[serde(rename = "audio.error")]
    Error { message: String },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioFrame {
    captured_at_unix_ms: u128,
    level: f32,
    peak: f32,
    bands: Vec<f32>,
}

struct Control {
    running: bool,
    interval: Duration,
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| format!("COM initialization failed: {error}"))?;
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

#[derive(Clone, Copy)]
enum SampleFormat {
    Float32,
    Pcm16,
    Pcm24,
    Pcm32,
}

struct AudioLoopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    channels: usize,
    sample_rate: u32,
    block_align: usize,
    format: SampleFormat,
}

impl AudioLoopback {
    fn open() -> Result<Self, String> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|error| format!("audio device enumerator failed: {error}"))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|error| {
                    format!("default Windows output device is unavailable: {error}")
                })?;
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|error| format!("WASAPI client activation failed: {error}"))?;
            let format_pointer = client
                .GetMixFormat()
                .map_err(|error| format!("WASAPI mix format failed: {error}"))?;
            if format_pointer.is_null() {
                return Err("WASAPI returned a null mix format".to_owned());
            }
            let parsed = parse_format(format_pointer);
            let initialized = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                2_000_000,
                0,
                format_pointer,
                None,
            );
            CoTaskMemFree(Some(format_pointer.cast()));
            initialized
                .map_err(|error| format!("WASAPI loopback initialization failed: {error}"))?;
            let (channels, sample_rate, block_align, format) = parsed?;
            let capture: IAudioCaptureClient = client
                .GetService()
                .map_err(|error| format!("WASAPI capture service failed: {error}"))?;
            client
                .Start()
                .map_err(|error| format!("WASAPI loopback start failed: {error}"))?;
            Ok(Self {
                client,
                capture,
                channels,
                sample_rate,
                block_align,
                format,
            })
        }
    }

    fn frame(&self) -> Result<AudioFrame, String> {
        let mut samples = Vec::<f32>::new();
        unsafe {
            loop {
                let packet_frames = self
                    .capture
                    .GetNextPacketSize()
                    .map_err(|error| format!("WASAPI packet query failed: {error}"))?;
                if packet_frames == 0 {
                    break;
                }
                let mut data = ptr::null_mut();
                let mut frames = 0_u32;
                let mut flags = 0_u32;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|error| format!("WASAPI packet read failed: {error}"))?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                if silent || data.is_null() {
                    samples.resize(samples.len() + frames as usize, 0.0);
                } else {
                    let bytes =
                        std::slice::from_raw_parts(data, frames as usize * self.block_align);
                    decode_mono(
                        bytes,
                        frames as usize,
                        self.channels,
                        self.block_align,
                        self.format,
                        &mut samples,
                    )?;
                }
                self.capture
                    .ReleaseBuffer(frames)
                    .map_err(|error| format!("WASAPI packet release failed: {error}"))?;
            }
        }
        if samples.len() > ANALYSIS_SAMPLES * 2 {
            samples.drain(..samples.len() - ANALYSIS_SAMPLES * 2);
        }
        Ok(analyze(&samples, self.sample_rate))
    }
}

impl Drop for AudioLoopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

fn main() -> Result<(), String> {
    if std::env::var("MYWALLPAPER_PROTOCOL").as_deref() != Ok("process-v2") {
        return Err("MYWALLPAPER_PROTOCOL must be process-v2".to_owned());
    }
    let output = Arc::new(Mutex::new(io::stdout()));
    let control = Arc::new((
        Mutex::new(Control {
            running: true,
            interval: Duration::from_millis(33),
        }),
        Condvar::new(),
    ));
    let mut input = io::stdin();
    let mut initialized = false;
    let mut sampler_thread = None;

    while let Some(frame) =
        read_json_record::<HostFrame>(&mut input).map_err(|error| error.to_string())?
    {
        let version = match &frame {
            HostFrame::Init { v, .. }
            | HostFrame::Settings { v, .. }
            | HostFrame::Message { v, .. }
            | HostFrame::Shutdown { v } => *v,
        };
        if version != PROTOCOL_VERSION {
            write_companion_error(
                &output,
                "protocol-version",
                format!("unsupported protocol version {version}"),
            )?;
            break;
        }
        match frame {
            HostFrame::Init {
                device_settings, ..
            } if !initialized => {
                set_interval(&control, &device_settings)?;
                write_frame(
                    &output,
                    &CompanionFrame::<Value>::Ready {
                        v: PROTOCOL_VERSION,
                    },
                )?;
                initialized = true;
                let thread_control = control.clone();
                let thread_output = output.clone();
                sampler_thread = Some(thread::spawn(move || {
                    run_sampler(thread_control, thread_output)
                }));
            }
            HostFrame::Settings {
                device_settings, ..
            } if initialized => {
                set_interval(&control, &device_settings)?;
            }
            HostFrame::Message { .. } if initialized => {}
            HostFrame::Shutdown { .. } => break,
            _ => {
                write_companion_error(
                    &output,
                    "protocol-state",
                    "invalid companion lifecycle frame".to_owned(),
                )?;
                break;
            }
        }
    }
    let (lock, wake) = &*control;
    let mut state = lock
        .lock()
        .map_err(|_| "control lock poisoned".to_owned())?;
    state.running = false;
    wake.notify_all();
    drop(state);
    if let Some(handle) = sampler_thread {
        handle
            .join()
            .map_err(|_| "audio sampler thread panicked".to_owned())?;
    }
    Ok(())
}

fn run_sampler(control: Arc<(Mutex<Control>, Condvar)>, output: Arc<Mutex<io::Stdout>>) {
    let _apartment = match ComApartment::initialize() {
        Ok(value) => value,
        Err(message) => {
            let _ = send_payload(&output, Payload::Error { message });
            return;
        }
    };
    let mut capture: Option<AudioLoopback> = None;
    loop {
        if capture.is_none() {
            match AudioLoopback::open() {
                Ok(value) => capture = Some(value),
                Err(message) => {
                    let _ = send_payload(&output, Payload::Error { message });
                }
            }
        }
        if let Some(active) = capture.as_ref() {
            match active.frame() {
                Ok(frame) => {
                    if send_payload(&output, Payload::Frame(frame)).is_err() {
                        return;
                    }
                }
                Err(message) => {
                    let _ = send_payload(&output, Payload::Error { message });
                    capture = None;
                }
            }
        }
        let (lock, wake) = &*control;
        let state = match lock.lock() {
            Ok(value) => value,
            Err(_) => return,
        };
        if !state.running {
            return;
        }
        let interval = state.interval;
        let (state, _) = match wake.wait_timeout(state, interval) {
            Ok(value) => value,
            Err(_) => return,
        };
        if !state.running {
            return;
        }
    }
}

unsafe fn parse_format(
    pointer: *const WAVEFORMATEX,
) -> Result<(usize, u32, usize, SampleFormat), String> {
    let format = unsafe { ptr::read_unaligned(pointer) };
    if format.nChannels == 0 || format.nBlockAlign == 0 || format.nSamplesPerSec == 0 {
        return Err("WASAPI mix format is incomplete".to_owned());
    }
    let sample_format = match (format.wFormatTag, format.wBitsPerSample) {
        (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleFormat::Float32,
        (WAVE_FORMAT_PCM, 16) => SampleFormat::Pcm16,
        (WAVE_FORMAT_PCM, 24) => SampleFormat::Pcm24,
        (WAVE_FORMAT_PCM, 32) => SampleFormat::Pcm32,
        (WAVE_FORMAT_EXTENSIBLE, bits) => {
            let extended = unsafe { ptr::read_unaligned(pointer.cast::<WAVEFORMATEXTENSIBLE>()) };
            let subformat = unsafe { ptr::addr_of!(extended.SubFormat).read_unaligned() };
            match (subformat, bits) {
                (KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, 32) => SampleFormat::Float32,
                (KSDATAFORMAT_SUBTYPE_PCM, 16) => SampleFormat::Pcm16,
                (KSDATAFORMAT_SUBTYPE_PCM, 24) => SampleFormat::Pcm24,
                (KSDATAFORMAT_SUBTYPE_PCM, 32) => SampleFormat::Pcm32,
                _ => {
                    return Err(format!(
                        "unsupported extensible WASAPI format ({bits} bits)"
                    ))
                }
            }
        }
        (tag, bits) => return Err(format!("unsupported WASAPI format tag {tag} ({bits} bits)")),
    };
    Ok((
        format.nChannels as usize,
        format.nSamplesPerSec,
        format.nBlockAlign as usize,
        sample_format,
    ))
}

fn decode_mono(
    bytes: &[u8],
    frames: usize,
    channels: usize,
    block_align: usize,
    format: SampleFormat,
    output: &mut Vec<f32>,
) -> Result<(), String> {
    let bytes_per_channel = block_align / channels;
    if bytes_per_channel == 0 || bytes.len() < frames.saturating_mul(block_align) {
        return Err("WASAPI packet length is invalid".to_owned());
    }
    output.reserve(frames);
    for frame in 0..frames {
        let mut mono = 0.0_f32;
        for channel in 0..channels {
            let offset = frame * block_align + channel * bytes_per_channel;
            let sample = match format {
                SampleFormat::Float32 => f32::from_le_bytes(
                    bytes[offset..offset + 4]
                        .try_into()
                        .map_err(|_| "truncated float sample")?,
                ),
                SampleFormat::Pcm16 => {
                    i16::from_le_bytes(
                        bytes[offset..offset + 2]
                            .try_into()
                            .map_err(|_| "truncated 16-bit sample")?,
                    ) as f32
                        / i16::MAX as f32
                }
                SampleFormat::Pcm24 => {
                    let raw = ((bytes[offset] as i32)
                        | ((bytes[offset + 1] as i32) << 8)
                        | ((bytes[offset + 2] as i32) << 16))
                        << 8
                        >> 8;
                    raw as f32 / 8_388_607.0
                }
                SampleFormat::Pcm32 => {
                    i32::from_le_bytes(
                        bytes[offset..offset + 4]
                            .try_into()
                            .map_err(|_| "truncated 32-bit sample")?,
                    ) as f32
                        / i32::MAX as f32
                }
            };
            mono += if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        output.push(mono / channels as f32);
    }
    Ok(())
}

fn analyze(samples: &[f32], sample_rate: u32) -> AudioFrame {
    let window = if samples.len() > ANALYSIS_SAMPLES {
        &samples[samples.len() - ANALYSIS_SAMPLES..]
    } else {
        samples
    };
    let mut squared = 0.0_f32;
    let mut peak = 0.0_f32;
    for sample in window {
        squared += sample * sample;
        peak = peak.max(sample.abs());
    }
    let level = if window.is_empty() {
        0.0
    } else {
        (squared / window.len() as f32).sqrt().clamp(0.0, 1.0)
    };
    let mut bands = vec![0.0_f32; SPECTRUM_BANDS];
    if window.len() >= 32 {
        for (band, value) in bands.iter_mut().enumerate() {
            let ratio = band as f32 / (SPECTRUM_BANDS - 1) as f32;
            let frequency = 40.0 * (18_000.0_f32 / 40.0).powf(ratio);
            let bin = (frequency * window.len() as f32 / sample_rate as f32).max(1.0);
            let mut real = 0.0_f32;
            let mut imaginary = 0.0_f32;
            for (index, sample) in window.iter().enumerate() {
                let hann = 0.5 - 0.5 * (TAU * index as f32 / (window.len() - 1) as f32).cos();
                let phase = TAU * bin * index as f32 / window.len() as f32;
                real += sample * hann * phase.cos();
                imaginary -= sample * hann * phase.sin();
            }
            *value = ((real * real + imaginary * imaginary).sqrt() * 4.0 / window.len() as f32)
                .clamp(0.0, 1.0);
        }
    }
    AudioFrame {
        captured_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or(0),
        level,
        peak,
        bands,
    }
}

fn set_interval(control: &Arc<(Mutex<Control>, Condvar)>, settings: &Value) -> Result<(), String> {
    let interval = match settings.get("refreshRate").and_then(Value::as_str) {
        Some("30fps") => Duration::from_millis(33),
        Some("60fps") => Duration::from_millis(16),
        _ => return Err("refreshRate must be 30fps or 60fps".to_owned()),
    };
    let (lock, wake) = &**control;
    let mut state = lock
        .lock()
        .map_err(|_| "control lock poisoned while applying settings".to_owned())?;
    state.interval = interval;
    wake.notify_all();
    Ok(())
}

fn send_payload(output: &Arc<Mutex<io::Stdout>>, payload: Payload) -> Result<(), String> {
    write_frame(
        output,
        &CompanionFrame::Message {
            v: PROTOCOL_VERSION,
            target: MessageTarget::Broadcast,
            payload,
        },
    )
}

fn write_frame<T: Serialize>(output: &Arc<Mutex<io::Stdout>>, value: &T) -> Result<(), String> {
    let mut output = output
        .lock()
        .map_err(|_| "output lock poisoned".to_owned())?;
    framing::write_json_record(&mut *output, value).map_err(|error| error.to_string())
}

fn write_companion_error(
    output: &Arc<Mutex<io::Stdout>>,
    code: &str,
    message: String,
) -> Result<(), String> {
    write_frame(
        output,
        &CompanionFrame::<Value>::Error {
            v: PROTOCOL_VERSION,
            message,
            code: code.to_owned(),
        },
    )
}
