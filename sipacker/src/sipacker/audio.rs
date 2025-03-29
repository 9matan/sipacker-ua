use std::error::Error;
use std::sync::Arc;

use bytes::Bytes;
use rubato::Resampler;

pub struct OutputStream {
    _output_stream: rodio::OutputStream,
    _output_stream_h: rodio::OutputStreamHandle,
}

impl OutputStream {
    pub fn build() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let (_output_stream, _output_stream_h) = rodio::OutputStream::try_default()?;
        Ok(Self {
            _output_stream,
            _output_stream_h,
        })
    }

    pub fn create_sink(&self) -> Result<OutputSink, Box<dyn Error + Send + Sync>> {
        Ok(OutputSink {
            sink: Arc::new(rodio::Sink::try_new(&self._output_stream_h)?),
        })
    }
}

pub struct OutputSink {
    sink: Arc<rodio::Sink>,
}

impl OutputSink {
    pub fn play_g711_alaw(&self, bytes: Bytes) {
        let data_len = bytes.len();
        let data = bytes
            .iter()
            .map(|&b| ezk_g711::alaw::decode(b) as f32 / i16::MAX as f32)
            .collect::<Vec<_>>();

        let mut resampler = rubato::FftFixedIn::<f32>::new(8000, 48000, data_len, 6, 1).unwrap();
        let data = resampler.process(&[data], None).unwrap().concat();

        let buffer = rodio::buffer::SamplesBuffer::new(1, 48000, data);

        self.sink.append(buffer);

        if self.sink.len() > 5 {
            self.sink.set_speed(1.1f32);
        } else {
            self.sink.set_speed(1.0f32);
        }
    }
}
