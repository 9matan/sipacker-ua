use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use tokio::sync::mpsc;

pub struct AudioSystem {
    _host: cpal::Host,
    out_device: Device<direction::Output>,
    in_device: Device<direction::Input>,
    stream_ch_buffer_size: usize,
}

struct Device<D> {
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    stream: Option<cpal::Stream>,
    direction: D,
}

impl AudioSystem {
    pub fn build() -> Result<Self, anyhow::Error> {
        let host = cpal::default_host();
        let out_device = Device::<direction::Output>::build_default(&host)?;
        let in_device = Device::<direction::Input>::build_default(&host)?;
        Ok(Self {
            _host: host,
            out_device,
            in_device,
            stream_ch_buffer_size: 200,
        })
    }

    pub fn create_output_stream(&mut self) -> Result<mpsc::Sender<bytes::Bytes>, anyhow::Error> {
        let (tx, rx) = mpsc::channel(self.stream_ch_buffer_size);
        self.out_device
            .create_stream(direction::Channel::Output(rx))?;
        tracing::info!("Output stream is created");
        Ok(tx)
    }

    pub fn destroy_output_stream(&mut self) {
        self.out_device.destroy_stream();
        tracing::info!("Output stream is destroyed");
    }

    pub fn create_input_stream(&mut self) -> Result<mpsc::Receiver<bytes::Bytes>, anyhow::Error> {
        let (tx, rx) = mpsc::channel(self.stream_ch_buffer_size);
        self.in_device
            .create_stream(direction::Channel::Input(tx))?;
        tracing::info!("Input stream is created");
        Ok(rx)
    }

    pub fn destroy_input_stream(&mut self) {
        self.in_device.destroy_stream();
        tracing::info!("Input stream is destroyed");
    }
}

impl<D: direction::DirectionTrait> Device<D> {
    fn destroy_stream(&mut self) {
        self.stream.take();
    }

    fn create_stream(&mut self, channel: direction::Channel) -> Result<(), anyhow::Error> {
        if self.stream.is_some() {
            return Err(anyhow::Error::msg(
                "Could not create a stream. It is already created",
            ));
        }

        let sample_format: cpal::SampleFormat = self.config.sample_format();
        let stream = match sample_format {
            cpal::SampleFormat::I8 => self.run_stream::<i8>(channel),
            cpal::SampleFormat::I16 => self.run_stream::<i16>(channel),
            cpal::SampleFormat::I32 => self.run_stream::<i32>(channel),
            cpal::SampleFormat::I64 => self.run_stream::<i64>(channel),
            cpal::SampleFormat::U8 => self.run_stream::<u8>(channel),
            cpal::SampleFormat::U16 => self.run_stream::<u16>(channel),
            cpal::SampleFormat::U32 => self.run_stream::<u32>(channel),
            cpal::SampleFormat::U64 => self.run_stream::<u64>(channel),
            cpal::SampleFormat::F32 => self.run_stream::<f32>(channel),
            cpal::SampleFormat::F64 => self.run_stream::<f64>(channel),
            sample_format => panic!("Unsupported sample format '{sample_format}'"),
        }?;
        self.stream = Some(stream);
        Ok(())
    }

    fn run_stream<T>(&self, channel: direction::Channel) -> Result<cpal::Stream>
    where
        T: cpal::SizedSample + dasp_sample::conv::ToSample<f32> + cpal::FromSample<f32> + Default,
    {
        let config = cpal::StreamConfig::from(self.config.clone());
        self.direction
            .build_stream::<T>(&self.device, config, channel)
    }
}

impl Device<direction::Input> {
    fn build_default(host: &cpal::Host) -> Result<Self, anyhow::Error> {
        let device = host
            .default_input_device()
            .ok_or(anyhow::Error::msg("Could not create input device"))?;
        let config = device.default_input_config()?;
        Ok(Self {
            device,
            config,
            stream: None,
            direction: direction::Input,
        })
    }
}

impl Device<direction::Output> {
    fn build_default(host: &cpal::Host) -> Result<Self, anyhow::Error> {
        let device = host
            .default_output_device()
            .ok_or(anyhow::Error::msg("Could not create output device"))?;
        let config = device.default_output_config()?;
        Ok(Self {
            device,
            config,
            stream: None,
            direction: direction::Output,
        })
    }
}

mod direction {
    use anyhow::Result;
    use cpal::{
        traits::{DeviceTrait, StreamTrait},
        Sample,
    };
    use rubato::Resampler;
    use tokio::sync::mpsc;

    pub enum Channel {
        Input(mpsc::Sender<bytes::Bytes>),
        Output(mpsc::Receiver<bytes::Bytes>),
    }

    pub trait DirectionTrait {
        fn build_stream<T>(
            &self,
            device: &cpal::Device,
            config: cpal::StreamConfig,
            channel: Channel,
        ) -> Result<cpal::Stream>
        where
            T: cpal::SizedSample
                + dasp_sample::conv::ToSample<f32>
                + cpal::FromSample<f32>
                + Default;
    }

    pub struct Input;
    pub struct Output;

    impl Input {
        fn read_stream_data<T>(
            input: &[T],
            channels: usize,
            sample_rate: usize,
            sender: &mut mpsc::Sender<bytes::Bytes>,
        ) where
            T: cpal::Sample + dasp_sample::conv::ToSample<f32>,
        {
            // read the first channel only
            let data = input
                .iter()
                .step_by(channels)
                .map(|i| i.to_sample())
                .collect();
            let data = resample_to_g711_alaw(data, sample_rate);
            let data = bytes::Bytes::from_iter(encode_g711_alaw(data));
            let _ = sender.try_send(data);
        }
    }

    impl DirectionTrait for Input {
        fn build_stream<T>(
            &self,
            device: &cpal::Device,
            config: cpal::StreamConfig,
            channel: Channel,
        ) -> Result<cpal::Stream>
        where
            T: cpal::SizedSample
                + dasp_sample::conv::ToSample<f32>
                + cpal::FromSample<f32>
                + Default,
        {
            let mut channel = if let Channel::Input(channel) = channel {
                channel
            } else {
                return Err(anyhow::Error::msg("The Input channel is expected"));
            };

            let channels = config.channels as usize;
            let sample_rate = config.sample_rate.0 as usize;
            let err_fn = |err| tracing::error!("an error occurred on input stream {err}");

            let stream = device.build_input_stream(
                &config,
                move |data: &[T], _: &cpal::InputCallbackInfo| {
                    Self::read_stream_data(data, channels, sample_rate, &mut channel)
                },
                err_fn,
                None,
            )?;
            stream.play()?;
            Ok(stream)
        }
    }

    impl Output {
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
                let data = decode_g711_alaw(bytes).collect();
                let data = resample_from_g711_alaw(data, sample_rate);

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

    impl DirectionTrait for Output {
        fn build_stream<T>(
            &self,
            device: &cpal::Device,
            config: cpal::StreamConfig,
            channel: Channel,
        ) -> Result<cpal::Stream>
        where
            T: cpal::SizedSample
                + dasp_sample::conv::ToSample<f32>
                + cpal::FromSample<f32>
                + Default,
        {
            let mut channel = if let Channel::Output(channel) = channel {
                channel
            } else {
                return Err(anyhow::Error::msg("The Input channel is expected"));
            };

            let channels = config.channels as usize;
            let sample_rate = config.sample_rate.0 as usize;
            let err_fn = |err| tracing::error!("an error occurred on output stream {err}");

            let stream = device.build_output_stream(
                &config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    Self::write_stream_data(data, channels, sample_rate, &mut channel)
                },
                err_fn,
                None,
            )?;
            stream.play()?;
            Ok(stream)
        }
    }

    fn decode_g711_alaw<I: IntoIterator<Item = u8>>(data: I) -> impl Iterator<Item = f32> {
        data.into_iter()
            .map(|d| ezk_g711::alaw::decode(d).to_sample())
    }

    fn encode_g711_alaw<T: std::borrow::Borrow<f32>, I: IntoIterator<Item = T>>(
        data: I,
    ) -> impl Iterator<Item = u8> {
        data.into_iter()
            .map(|d| ezk_g711::alaw::encode(d.borrow().to_sample()))
    }

    fn resample_from_g711_alaw(data: Vec<f32>, sample_rate_out: usize) -> Vec<f32> {
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

    fn resample_to_g711_alaw(data: Vec<f32>, sample_rate_in: usize) -> Vec<f32> {
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
