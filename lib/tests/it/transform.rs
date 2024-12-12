use async_compression::tokio::bufread::{GzipEncoder, ZstdEncoder};
use circe_lib::{
    transform::{self, Chunk},
    LayerMediaTypeFlag,
};
use color_eyre::Result;
use futures_lite::Stream;
use simple_test_case::test_case;
use std::io::Cursor;
use tokio_util::io::{ReaderStream, StreamReader};

#[test_case(b"Hello, World!"; "hello_world")]
#[test_log::test(tokio::test)]
async fn identity(input: &[u8]) -> Result<()> {
    let stream = stream(input);
    let transformed = transform::identity(stream);
    let result = buffer(transformed).await?;
    assert_eq!(result, input);
    Ok(())
}

#[test_case(b"Hello, World!"; "hello_world")]
#[test_log::test(tokio::test)]
async fn gzip(input: &[u8]) -> Result<()> {
    let compressed = gzip(input).await?;
    let stream = stream(&compressed);
    let transformed = transform::gzip(stream);
    let result = buffer(transformed).await?;
    assert_eq!(result, input);
    Ok(())
}

#[test_case(b"Hello, World!"; "hello_world")]
#[test_log::test(tokio::test)]
async fn zstd(input: &[u8]) -> Result<()> {
    let compressed = zstd(input).await?;
    let stream = stream(&compressed);
    let transformed = transform::zstd(stream);
    let result = buffer(transformed).await?;
    assert_eq!(result, input);
    Ok(())
}

#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Zstd]; "hello_world_zstd")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Gzip]; "hello_world_gzip")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Zstd, LayerMediaTypeFlag::Gzip]; "hello_world_zstd_gzip")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Gzip, LayerMediaTypeFlag::Zstd]; "hello_world_gzip_zstd")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Zstd, LayerMediaTypeFlag::Gzip, LayerMediaTypeFlag::Foreign]; "hello_world_zstd_gzip_foreign")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Gzip, LayerMediaTypeFlag::Zstd, LayerMediaTypeFlag::Foreign]; "hello_world_gzip_zstd_foreign")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Zstd, LayerMediaTypeFlag::Foreign, LayerMediaTypeFlag::Gzip]; "hello_world_zstd_foreign_gzip")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Gzip, LayerMediaTypeFlag::Foreign, LayerMediaTypeFlag::Zstd]; "hello_world_gzip_foreign_zstd")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Foreign, LayerMediaTypeFlag::Zstd, LayerMediaTypeFlag::Gzip]; "hello_world_foreign_zstd_gzip")]
#[test_case(b"Hello, World!", &[LayerMediaTypeFlag::Foreign, LayerMediaTypeFlag::Gzip, LayerMediaTypeFlag::Zstd]; "hello_world_foreign_gzip_zstd")]
#[test_log::test(tokio::test)]
async fn flags(input: &[u8], flags: &[LayerMediaTypeFlag]) -> Result<()> {
    use color_eyre::eyre::Context;

    // Apply the flags to the input in the same order as we'll transform them.
    let mut compressed = input.to_vec();
    for flag in flags.iter().rev() {
        match flag {
            LayerMediaTypeFlag::Zstd => {
                compressed = zstd(&compressed).await.context("apply zstd")?;
            }
            LayerMediaTypeFlag::Gzip => {
                compressed = gzip(&compressed).await.context("apply gzip")?;
            }
            LayerMediaTypeFlag::Foreign => {
                compressed = identity(&compressed).await.context("apply identity")?;
            }
        }
    }

    let stream = stream(&compressed);
    let transformed = transform::sequence(stream, flags);
    let result = buffer(transformed).await.context("buffer stream")?;
    assert_eq!(result, input);
    Ok(())
}

fn stream(data: &[u8]) -> impl Stream<Item = Chunk> {
    let data = data.to_vec();
    let data = Cursor::new(data);
    ReaderStream::new(data)
}

async fn buffer(stream: impl Stream<Item = Chunk> + Unpin) -> Result<Vec<u8>> {
    let mut reader = StreamReader::new(stream);
    let mut buffer = Vec::new();
    tokio::io::copy(&mut reader, &mut buffer).await?;
    Ok(buffer)
}

async fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzipEncoder::new(data);
    let mut compressed = Vec::new();
    tokio::io::copy(&mut encoder, &mut compressed).await?;
    Ok(compressed)
}

async fn zstd(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZstdEncoder::new(data);
    let mut compressed = Vec::new();
    tokio::io::copy(&mut encoder, &mut compressed).await?;
    Ok(compressed)
}

async fn identity(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}
