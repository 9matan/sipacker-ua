use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

pub struct AudioSystem {
    _host: cpal::Host,
    out_device: AudioOutDevice,
    in_device: AudioInDevice,
}

struct AudioOutDevice {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    stream: Option<cpal::Stream>,
}

struct AudioInDevice {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    stream: Option<cpal::Stream>,
}

impl AudioSystem {
    pub fn build() -> Result<Self, anyhow::Error> {
        let host = cpal::default_host();
        let out_device = AudioOutDevice::build(&host)?;
        let in_device = AudioInDevice::build(&host)?;
        Ok(Self {
            _host: host,
            out_device,
            in_device,
        })
    }

    pub fn create_output_stream(&mut self) -> Result<mpsc::Sender<bytes::Bytes>, anyhow::Error> {
        let (tx, rx) = mpsc::channel(100);
        self.out_device.create_stream(rx)?;
        Ok(tx)
    }

    pub fn destroy_output_stream(&mut self) {
        self.out_device.destroy_stream();
    }

    pub fn create_input_stream(&mut self) -> Result<mpsc::Receiver<bytes::Bytes>, anyhow::Error> {
        let (tx, rx) = mpsc::channel(100);
        self.in_device.create_stream(tx)?;
        Ok(rx)
    }

    pub fn destroy_input_stream(&mut self) {
        self.in_device.destroy_stream();
    }
}

impl AudioOutDevice {
    fn build(host: &cpal::Host) -> Result<Self, anyhow::Error> {
        let device = host
            .default_output_device()
            .ok_or(anyhow::Error::msg("Could not create output device"))?;
        let config = device.default_output_config()?;
        Ok(Self {
            device,
            config,
            stream: None,
        })
    }

    fn destroy_stream(&mut self) {
        self.stream.take();
    }

    fn create_stream(
        &mut self,
        receiver: mpsc::Receiver<bytes::Bytes>,
    ) -> Result<(), anyhow::Error> {
        let sample_format = self.config.sample_format();
        let stream = match sample_format {
            cpal::SampleFormat::I8 => self.run_stream::<i8>(receiver),
            cpal::SampleFormat::I16 => self.run_stream::<i16>(receiver),
            cpal::SampleFormat::I32 => self.run_stream::<i32>(receiver),
            cpal::SampleFormat::I64 => self.run_stream::<i64>(receiver),
            cpal::SampleFormat::U8 => self.run_stream::<u8>(receiver),
            cpal::SampleFormat::U16 => self.run_stream::<u16>(receiver),
            cpal::SampleFormat::U32 => self.run_stream::<u32>(receiver),
            cpal::SampleFormat::U64 => self.run_stream::<u64>(receiver),
            cpal::SampleFormat::F32 => self.run_stream::<f32>(receiver),
            cpal::SampleFormat::F64 => self.run_stream::<f64>(receiver),
            sample_format => panic!("Unsupported sample format '{sample_format}'"),
        }?;
        self.stream = Some(stream);
        Ok(())
    }

    fn run_stream<T>(
        &mut self,
        mut receiver: mpsc::Receiver<bytes::Bytes>,
    ) -> Result<cpal::Stream, anyhow::Error>
    where
        T: cpal::SizedSample + cpal::FromSample<f32> + Default,
    {
        let config = cpal::StreamConfig::from(self.config.clone());
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate.0 as usize;
        let err_fn = |err| tracing::error!("an error occurred on output stream {err}");

        let stream = self.device.build_output_stream(
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
        receiver: &mut mpsc::Receiver<bytes::Bytes>,
    ) where
        T: cpal::Sample + cpal::FromSample<f32> + Default,
    {
        let mut buffer = Vec::new();
        while let Ok(bytes) = receiver.try_recv() {
            let data = miscs::decode_g711_alaw(bytes).collect();
            let data = miscs::resample_from_g711_alaw(data, sample_rate);

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

impl AudioInDevice {
    fn build(host: &cpal::Host) -> Result<Self, anyhow::Error> {
        let device = host
            .default_input_device()
            .ok_or(anyhow::Error::msg("Could not create input device"))?;
        let config = device.default_input_config()?;
        Ok(Self {
            device,
            config,
            stream: None,
        })
    }

    fn destroy_stream(&mut self) {
        self.stream.take();
    }

    fn create_stream(&mut self, sender: mpsc::Sender<bytes::Bytes>) -> Result<(), anyhow::Error> {
        let sample_format = self.config.sample_format();
        let stream = match sample_format {
            cpal::SampleFormat::I8 => self.run_stream::<i8>(sender),
            cpal::SampleFormat::I16 => self.run_stream::<i16>(sender),
            cpal::SampleFormat::I32 => self.run_stream::<i32>(sender),
            cpal::SampleFormat::I64 => self.run_stream::<i64>(sender),
            cpal::SampleFormat::U8 => self.run_stream::<u8>(sender),
            cpal::SampleFormat::U16 => self.run_stream::<u16>(sender),
            cpal::SampleFormat::U32 => self.run_stream::<u32>(sender),
            cpal::SampleFormat::U64 => self.run_stream::<u64>(sender),
            cpal::SampleFormat::F32 => self.run_stream::<f32>(sender),
            cpal::SampleFormat::F64 => self.run_stream::<f64>(sender),
            sample_format => panic!("Unsupported sample format '{sample_format}'"),
        }?;
        self.stream = Some(stream);
        Ok(())
    }

    fn run_stream<T>(
        &mut self,
        mut sender: mpsc::Sender<bytes::Bytes>,
    ) -> Result<cpal::Stream, anyhow::Error>
    where
        T: cpal::SizedSample + dasp_sample::conv::ToSample<f32>,
    {
        let config = cpal::StreamConfig::from(self.config.clone());
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate.0 as usize;
        let err_fn = |err| tracing::error!("an error occurred on input stream {err}");

        let stream = self.device.build_input_stream(
            &config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                Self::read_stream_data(data, channels, sample_rate, &mut sender)
            },
            err_fn,
            None,
        )?;
        stream.play()?;
        Ok(stream)
    }

    fn read_stream_data<T>(
        input: &[T],
        channels: usize,
        sample_rate: usize,
        sender: &mut mpsc::Sender<bytes::Bytes>,
    ) where
        T: cpal::Sample + dasp_sample::conv::ToSample<f32>,
    {
        let data = input
            .iter()
            .step_by(channels)
            .map(|i| i.to_sample())
            .collect();
        let data = miscs::resample_to_g711_alaw(data, sample_rate);
        let data = bytes::Bytes::from_iter(miscs::encode_g711_alaw(data));
        let _ = sender.try_send(data);
    }
}

mod miscs {
    use cpal::Sample;
    use rubato::Resampler;

    pub fn decode_g711_alaw<I: IntoIterator<Item = u8>>(data: I) -> impl Iterator<Item = f32> {
        data.into_iter()
            .map(|d| ezk_g711::alaw::decode(d).to_sample())
    }

    pub fn encode_g711_alaw<T: std::borrow::Borrow<f32>, I: IntoIterator<Item = T>>(
        data: I,
    ) -> impl Iterator<Item = u8> {
        data.into_iter()
            .map(|d| ezk_g711::alaw::encode(d.borrow().to_sample()))
    }

    pub fn resample_from_g711_alaw(data: Vec<f32>, sample_rate_out: usize) -> Vec<f32> {
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

    pub fn resample_to_g711_alaw(data: Vec<f32>, sample_rate_in: usize) -> Vec<f32> {
        let sample_rate = 8000;
        let sub_chunks = 4;
        let channels_count = 1;
        let mut resampler = rubato::FftFixedIn::<f32>::new(
            sample_rate_in,
            sample_rate,
            data.len(),
            sub_chunks,
            channels_count,
        )
        .unwrap();
        resampler.process(&[data], None).unwrap().concat()
    }
}
