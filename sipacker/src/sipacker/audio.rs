use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ezk_rtp::RtpPacket;
use tokio::sync::mpsc;

pub struct AudioSystem {
    host: cpal::Host,
    out_device: AudioOutDevice,
}

struct AudioOutDevice {
    device: cpal::Device,
    stream: cpal::Stream,
}

impl AudioSystem {
    pub fn build(
        output_receiver: mpsc::Receiver<RtpPacket>, /*, _intput_sender: mpsc::Sender<RtpPacket>*/
    ) -> Result<Self, anyhow::Error> {
        let host = cpal::default_host();
        let out_device = AudioOutDevice::build(&host, output_receiver)?;
        let out_device = out_device;
        Ok(Self { host, out_device })
    }
}

impl AudioOutDevice {
    fn build(
        host: &cpal::Host,
        receiver: mpsc::Receiver<RtpPacket>,
    ) -> Result<Self, anyhow::Error> {
        let mut device = host
            .default_output_device()
            .ok_or(anyhow::Error::msg("Could not create output device"))?;
        let stream = Self::build_stream(&mut device, receiver)?;
        Ok(Self { device, stream })
    }

    fn build_stream(
        device: &mut cpal::Device,
        receiver: mpsc::Receiver<RtpPacket>,
    ) -> Result<cpal::Stream, anyhow::Error> {
        let config = device.default_output_config()?;
        let sample_format = config.sample_format();
        let mut config = cpal::StreamConfig::from(config);
        config.channels = 1;
        match sample_format {
            cpal::SampleFormat::I8 => Self::run_srteam::<i8>(device, &config, receiver),
            cpal::SampleFormat::I16 => Self::run_srteam::<i16>(device, &config, receiver),
            cpal::SampleFormat::I32 => Self::run_srteam::<i32>(device, &config, receiver),
            cpal::SampleFormat::I64 => Self::run_srteam::<i64>(device, &config, receiver),
            cpal::SampleFormat::U8 => Self::run_srteam::<u8>(device, &config, receiver),
            cpal::SampleFormat::U16 => Self::run_srteam::<u16>(device, &config, receiver),
            cpal::SampleFormat::U32 => Self::run_srteam::<u32>(device, &config, receiver),
            cpal::SampleFormat::U64 => Self::run_srteam::<u64>(device, &config, receiver),
            cpal::SampleFormat::F32 => Self::run_srteam::<f32>(device, &config, receiver),
            cpal::SampleFormat::F64 => Self::run_srteam::<f64>(device, &config, receiver),
            sample_format => panic!("Unsupported sample format '{sample_format}'"),
        }
    }

    pub fn run_srteam<T>(
        device: &mut cpal::Device,
        config: &cpal::StreamConfig,
        mut receiver: mpsc::Receiver<RtpPacket>,
    ) -> Result<cpal::Stream, anyhow::Error>
    where
        T: cpal::SizedSample + cpal::FromSample<f32> + Default,
    {
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate.0 as usize;
        let err_fn = |err| log::error!(err:%; "an error occurred on stream");

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                Self::write_stream_data(data, channels, sample_rate, &mut receiver)
            },
            err_fn,
            None,
        )?;
        stream.play()?;
        Ok(stream)
    }

    fn write_stream_data<T>(
        output: &mut [T],
        channels: usize,
        sample_rate: usize,
        receiver: &mut mpsc::Receiver<RtpPacket>,
    ) where
        T: cpal::Sample + cpal::FromSample<f32> + Default,
    {
        let mut buffer = Vec::new();
        while let Ok(packet) = receiver.try_recv() {
            let data = miscs::decode_g711_alaw(packet.payload);
            let data = miscs::resample_g711_alaw(data, sample_rate);

            buffer.extend(data);
            if buffer.len() >= output.len() {
                break;
            }
        }

        output.fill(T::default());
        buffer.reverse();
        for frame in output.chunks_mut(channels) {
            if let Some(s) = buffer.pop() {
                frame.fill(T::from_sample_(s));
            }
        }
    }
}

mod miscs {
    use rubato::Resampler;

    pub fn decode_g711_alaw(bytes: bytes::Bytes) -> Vec<f32> {
        bytes
            .iter()
            .map(|&b| ezk_g711::alaw::decode(b) as f32 / i16::MAX as f32)
            .collect()
    }

    pub fn resample_g711_alaw(data: Vec<f32>, sample_rate_out: usize) -> Vec<f32> {
        let sample_rate = 8000;
        let sub_chunks = 4;
        let channels_count = 1;
        let mut resampler = rubato::FftFixedIn::<f32>::new(
            sample_rate,
            sample_rate_out,
            data.len(),
            sub_chunks,
            channels_count,
        )
        .unwrap();
        resampler.process(&[data], None).unwrap().concat()
    }
}
