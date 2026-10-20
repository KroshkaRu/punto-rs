//! Сигнал и клиент PulseAudio включены в бинарник; внешнего проигрывателя нет.
use anyhow::{Context, Result};
use pulseaudio::{AsPlaybackSource, Client, protocol};
use std::{os::unix::net::UnixStream, time::Duration};

const RATE: u32 = 44_100;

fn signal() -> Vec<u8> {
    let length = RATE as usize / 10;
    (0..length)
        .flat_map(|index| {
            // Короткие края убирают щелчки при запуске и завершении сигнала.
            let fade = (index.min(length - 1 - index) as f32 / 220.0).min(1.0);
            let phase = std::f32::consts::TAU * 880.0 * index as f32 / RATE as f32;
            ((phase.sin() * fade * 5000.0) as i16).to_le_bytes()
        })
        .collect()
}

pub fn play() -> Result<()> {
    let path = pulseaudio::socket_path_from_env()
        .context("Нет доступного сервера PulseAudio / PipeWire Pulse")?;
    let socket = UnixStream::connect(path).context("Подключение к аудиосерверу")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    // Закрытие сокета также останавливает внутренний поток клиента при тайм-ауте.
    let shutdown = socket.try_clone()?;
    // Протокол требует 256 байт даже при авторизации по UID сокета (PipeWire).
    let cookie = pulseaudio::cookie_path_from_env()
        .and_then(|path| std::fs::read(path).ok())
        .filter(|cookie| cookie.len() == 256)
        .unwrap_or_else(|| vec![0; 256]);
    let client = Client::new_unix(c"Punto RS", socket, Some(cookie))?;
    let samples = signal();
    let mut position = 0;
    let source = move |buffer: &mut [u8]| {
        let count = buffer.len().min(samples.len() - position);
        buffer[..count].copy_from_slice(&samples[position..position + count]);
        position += count;
        count
    };
    let result = futures_lite::future::block_on(futures_lite::future::race(
        async {
            let params = protocol::PlaybackStreamParams {
                sample_spec: protocol::SampleSpec {
                    format: protocol::SampleFormat::S16Le,
                    channels: 1,
                    sample_rate: RATE,
                },
                buffer_attr: protocol::stream::BufferAttr {
                    target_length: RATE / 10,
                    pre_buffering: RATE / 50,
                    minimum_request_length: RATE / 50,
                    ..Default::default()
                },
                flags: protocol::stream::StreamFlags {
                    adjust_latency: true,
                    ..Default::default()
                },
                channel_map: protocol::ChannelMap::mono(),
                cvolume: Some(protocol::ChannelVolume::norm(1)),
                sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
                ..Default::default()
            };
            let stream = client
                .create_playback_stream(params, source.as_playback_source())
                .await?;
            stream.play_all().await?;
            Ok(())
        },
        async {
            async_io::Timer::after(Duration::from_secs(3)).await;
            anyhow::bail!("Аудиосервер не завершил воспроизведение за 3 секунды")
        },
    ));
    let _ = shutdown.shutdown(std::net::Shutdown::Both);
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn signal_is_audible_and_has_silent_edges() {
        let pcm = super::signal();
        let samples: Vec<_> = pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
            .collect();
        assert_eq!(samples.len(), 4410);
        assert_eq!(samples[0], 0);
        assert_eq!(*samples.last().unwrap(), 0);
        assert!(samples.iter().any(|sample| sample.abs() > 4000));
        assert!(samples.iter().all(|sample| sample.abs() <= 5000));
    }
}
