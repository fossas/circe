//! Primitives for stream transformations.

use std::pin::Pin;

use async_compression::tokio::bufread::{GzipDecoder, ZstdDecoder};
use bytes::Bytes;
use color_eyre::Result;
use futures_lite::Stream;
use tokio_util::io::{ReaderStream, StreamReader};

use crate::LayerMediaTypeFlag;

/// Convenience alias for a chunk of bytes in a stream.
pub type Chunk = Result<Bytes, std::io::Error>;

/// Identity transformer.
pub fn identity(stream: impl Stream<Item = Chunk> + Send + 'static) -> impl Stream<Item = Chunk> + Send {
    stream
}

/// Decompress the stream using gzip.
pub fn gzip(stream: impl Stream<Item = Chunk> + Send + 'static) -> impl Stream<Item = Chunk> + Send {
    let reader = StreamReader::new(stream);
    let inner = GzipDecoder::new(reader);
    ReaderStream::new(inner)
}

/// Decompress the stream using zstd.
pub fn zstd(stream: impl Stream<Item = Chunk> + Send + 'static) -> impl Stream<Item = Chunk> + Send {
    let reader = StreamReader::new(stream);
    let inner = ZstdDecoder::new(reader);
    ReaderStream::new(inner)
}

/// Apply a sequence of transformations to the stream based on the media type flags.
pub fn sequence(
    stream: impl Stream<Item = Chunk> + Send + 'static,
    flags: &[LayerMediaTypeFlag],
) -> Pin<Box<dyn Stream<Item = Chunk> + Send>> {
    // Left hand side type annotation is required to coerce to dynamic dispatching.
    let mut stream: Pin<Box<dyn Stream<Item = Chunk> + Send>> = Box::pin(stream);

    // Each flag in order consumes the prior stream, replacing it with a new transformed stream.
    for flag in flags {
        match flag {
            LayerMediaTypeFlag::Zstd => stream = Box::pin(zstd(stream)),
            LayerMediaTypeFlag::Gzip => stream = Box::pin(gzip(stream)),
            _ => (),
        }
    }

    // The final stream is therefore the sequenced version of the stream.
    stream
}
