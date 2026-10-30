use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{Error, Result};

/// Random-access byte source used by parsers and lazy asset payloads.
pub trait Source: Send + Sync {
    fn len(&self) -> u64;

    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()>;

    /// Optional read-ahead for an immutable metadata scan. Random-access and
    /// payload readers use `read_exact_at` and keep their normal I/O behavior.
    fn read_exact_at_with_read_ahead(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        self.read_exact_at(offset, output)
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A source-bound, strictly bounded byte range.
///
/// Cloning a region is cheap. Cursors created from separate clones have
/// independent logical positions and may be consumed concurrently.
#[derive(Clone)]
pub struct Region {
    source: Arc<dyn Source>,
    source_offset: u64,
    length: u64,
}

impl Region {
    pub fn new(source: Arc<dyn Source>, source_offset: u64, length: u64) -> Result<Self> {
        let end = source_offset
            .checked_add(length)
            .ok_or_else(|| Error::invalid_data("source region range overflowed"))?;
        if end > source.len() {
            return Err(Error::invalid_data(format!(
                "source region {source_offset}..{end} exceeds source length {}",
                source.len()
            )));
        }
        Ok(Self {
            source,
            source_offset,
            length,
        })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        Self::new(
            Arc::new(ReadAheadSource::new(Arc::new(FileSource::new(
                file, length,
            )))),
            0,
            length,
        )
    }

    /// Concatenates bounded regions into one lazy random-access source.
    ///
    /// Segment bytes remain in their original sources and are read only when a
    /// consumer crosses the corresponding logical range.
    pub fn concatenate(segments: Vec<Self>, maximum_length: u64) -> Result<Self> {
        if segments.is_empty() {
            return Err(Error::invalid_data(
                "segmented source requires at least one region",
            ));
        }
        let source = SegmentedSource::new(segments, maximum_length)?;
        let length = source.length;
        Self::new(Arc::new(source), 0, length)
    }

    #[must_use]
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::from_memory(MemorySource {
            bytes: bytes.into(),
        })
    }

    /// [`Self::from_bytes`] for an owned buffer, taking it over in place.
    ///
    /// `Vec<u8>` into `Arc<[u8]>` allocates and copies the whole buffer, so
    /// the decompressed entries and blocks this crate produces would briefly
    /// exist twice.
    #[must_use]
    pub(crate) fn from_vec(bytes: Vec<u8>) -> Self {
        Self::from_memory(MemorySource { bytes })
    }

    fn from_memory<B: AsRef<[u8]> + Send + Sync + 'static>(source: MemorySource<B>) -> Self {
        let source = Arc::new(source);
        Self {
            length: source.len(),
            source,
            source_offset: 0,
        }
    }

    #[must_use]
    pub const fn len(&self) -> u64 {
        self.length
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    #[must_use]
    pub const fn source_offset(&self) -> u64 {
        self.source_offset
    }

    pub fn subregion(&self, offset: u64, length: u64) -> Result<Self> {
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Error::invalid_data("subregion range overflowed"))?;
        if end > self.length {
            return Err(Error::invalid_data(format!(
                "subregion {offset}..{end} exceeds region length {}",
                self.length
            )));
        }
        let source_offset = self
            .source_offset
            .checked_add(offset)
            .ok_or_else(|| Error::invalid_data("subregion source offset overflowed"))?;
        Self::new(Arc::clone(&self.source), source_offset, length)
    }

    #[must_use]
    pub fn cursor(&self) -> RegionCursor {
        RegionCursor {
            region: self.clone(),
            position: 0,
            buffer: None,
            read_ahead: false,
        }
    }

    /// Small metadata reads share a bounded read-ahead window. Logical seeks,
    /// including length checks, preserve that window; bulk payload reads bypass
    /// it. The immutable source and this region's bounds remain authoritative.
    pub(crate) fn buffered_cursor(&self, capacity: usize) -> RegionCursor {
        let mut cursor = self.cursor();
        cursor.buffer = Some(CursorBuffer {
            bytes: Vec::new(),
            start: 0,
            filled: 0,
            capacity: capacity
                .clamp(1, 64 * 1024)
                .min(usize::try_from(self.len()).unwrap_or(usize::MAX)),
        });
        cursor
    }

    pub(crate) fn read_ahead_cursor(&self, capacity: usize) -> RegionCursor {
        let mut cursor = self.buffered_cursor(capacity);
        cursor.read_ahead = true;
        cursor
    }

    pub fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> Result<()> {
        self.read_exact_at_io(offset, output)?;
        Ok(())
    }

    pub fn copy_range<W: Write>(&self, offset: u64, length: u64, output: &mut W) -> Result<u64> {
        self.check_range(offset, length)?;
        let mut copied = 0_u64;
        let mut buffer = [0_u8; 16 * 1024];
        while copied < length {
            let remaining = length - copied;
            let chunk_length = usize::try_from(
                remaining.min(u64::try_from(buffer.len()).expect("buffer length fits in u64")),
            )
            .expect("chunk length is bounded by the fixed buffer");
            self.read_exact_at(offset + copied, &mut buffer[..chunk_length])?;
            output.write_all(&buffer[..chunk_length])?;
            copied += u64::try_from(chunk_length).expect("chunk length fits in u64");
        }
        Ok(copied)
    }

    pub fn read_to_vec(&self, maximum_length: u64) -> Result<Vec<u8>> {
        if self.length > maximum_length {
            return Err(Error::invalid_data(format!(
                "region is {} bytes, exceeding materialization limit {maximum_length}",
                self.length
            )));
        }
        let length = usize::try_from(self.length)
            .map_err(|_| Error::invalid_data("region is too large for this platform"))?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(length).map_err(|error| {
            Error::invalid_data(format!("cannot allocate {length} region bytes: {error}"))
        })?;
        bytes.resize(length, 0);
        self.read_exact_at(0, &mut bytes)?;
        Ok(bytes)
    }

    fn check_range(&self, offset: u64, length: u64) -> Result<()> {
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Error::invalid_data("region read range overflowed"))?;
        if end > self.length {
            return Err(Error::invalid_data(format!(
                "region read {offset}..{end} exceeds length {}",
                self.length
            )));
        }
        Ok(())
    }

    fn read_exact_at_io(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        self.read_exact_at_io_with_hint(offset, output, false)
    }

    fn read_exact_at_io_with_hint(
        &self,
        offset: u64,
        output: &mut [u8],
        read_ahead: bool,
    ) -> io::Result<()> {
        let length = u64::try_from(output.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "buffer length overflowed"))?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read range overflowed"))?;
        if end > self.length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("region read {offset}..{end} exceeds length {}", self.length),
            ));
        }
        let source_offset = self.source_offset.checked_add(offset).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source offset overflowed")
        })?;
        if read_ahead {
            self.source
                .read_exact_at_with_read_ahead(source_offset, output)
        } else {
            self.source.read_exact_at(source_offset, output)
        }
    }
}

impl fmt::Debug for Region {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Region")
            .field("source_offset", &self.source_offset)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Region {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
            && self.source_offset == other.source_offset
            && self.length == other.length
    }
}

impl Eq for Region {}

pub struct RegionCursor {
    region: Region,
    position: u64,
    buffer: Option<CursorBuffer>,
    read_ahead: bool,
}

struct CursorBuffer {
    bytes: Vec<u8>,
    start: u64,
    filled: usize,
    capacity: usize,
}

impl RegionCursor {
    #[must_use]
    pub const fn region(&self) -> &Region {
        &self.region
    }

    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }
}

impl Read for RegionCursor {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let remaining = self.region.length - self.position;
        let count = usize::try_from(remaining.min(u64::try_from(output.len()).unwrap_or(u64::MAX)))
            .expect("read count is bounded by the output slice");
        if count == 0 {
            return Ok(0);
        }
        let copied = if let Some(buffer) = &mut self.buffer
            && count < buffer.capacity
        {
            if self.position < buffer.start || self.position >= buffer.start + buffer.filled as u64
            {
                let fill = buffer
                    .capacity
                    .min(usize::try_from(remaining).unwrap_or(usize::MAX));
                buffer.filled = 0;
                if buffer.bytes.len() < fill {
                    buffer
                        .bytes
                        .try_reserve_exact(fill - buffer.bytes.len())
                        .map_err(io::Error::other)?;
                    buffer.bytes.resize(fill, 0);
                }
                self.region.read_exact_at_io_with_hint(
                    self.position,
                    &mut buffer.bytes[..fill],
                    self.read_ahead,
                )?;
                buffer.start = self.position;
                buffer.filled = fill;
            }
            let offset =
                usize::try_from(self.position - buffer.start).expect("cache offset is bounded");
            let copied = count.min(buffer.filled - offset);
            output[..copied].copy_from_slice(&buffer.bytes[offset..offset + copied]);
            copied
        } else {
            self.region.read_exact_at_io_with_hint(
                self.position,
                &mut output[..count],
                self.read_ahead,
            )?;
            count
        };
        self.position += u64::try_from(copied).expect("read count fits in u64");
        Ok(copied)
    }
}

impl Seek for RegionCursor {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.region.length) + i128::from(delta),
        };
        if target < 0 || target > i128::from(self.region.length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek target is outside the bounded region",
            ));
        }
        self.position = u64::try_from(target).expect("validated seek target fits in u64");
        Ok(self.position)
    }
}

/// A file read by absolute offset.
///
/// Parsing a serialized file is thousands of small reads, and pairing a seek
/// with each one costs a second syscall every time and forces the shared file
/// position under a lock. Unix `pread` takes the offset directly and leaves the
/// position alone, so neither is needed there. Platforms without it keep the
/// seek, and keep the lock that the shared position then requires.
struct PositionalFile {
    #[cfg(unix)]
    file: File,
    #[cfg(not(unix))]
    file: Mutex<File>,
}

impl PositionalFile {
    #[cfg(unix)]
    const fn new(file: File) -> Self {
        Self { file }
    }

    #[cfg(not(unix))]
    const fn new(file: File) -> Self {
        Self {
            file: Mutex::new(file),
        }
    }

    #[cfg(unix)]
    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        std::os::unix::fs::FileExt::read_exact_at(&self.file, output, offset)
    }

    #[cfg(not(unix))]
    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("source file lock was poisoned"))?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(output)
    }
}

struct FileSource {
    file: PositionalFile,
    length: u64,
}

/// Object-name readers are short-lived, but adjacent objects often occupy the
/// same file pages. Share one bounded read-ahead window per physical source so
/// those readers reuse file reads. Large payload reads bypass this window.
struct ReadAheadSource {
    source: Arc<dyn Source>,
    window: Mutex<SourceWindow>,
}

#[derive(Default)]
struct SourceWindow {
    bytes: Vec<u8>,
    start: u64,
    filled: usize,
}

impl ReadAheadSource {
    const CAPACITY: usize = 64 * 1024;
    const SMALL_READ: usize = 4 * 1024;

    fn new(source: Arc<dyn Source>) -> Self {
        Self {
            source,
            window: Mutex::new(SourceWindow::default()),
        }
    }
}

impl Source for ReadAheadSource {
    fn len(&self) -> u64 {
        self.source.len()
    }

    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        self.source.read_exact_at(offset, output)
    }

    fn read_exact_at_with_read_ahead(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        if output.is_empty() || output.len() > Self::SMALL_READ {
            return self.source.read_exact_at(offset, output);
        }
        let mut window = self
            .window
            .lock()
            .map_err(|_| io::Error::other("file read-ahead lock was poisoned"))?;
        let hit = offset
            .checked_sub(window.start)
            .and_then(|position| usize::try_from(position).ok())
            .filter(|&position| {
                position
                    .checked_add(output.len())
                    .is_some_and(|end| end <= window.filled)
            });
        if let Some(position) = hit {
            output.copy_from_slice(&window.bytes[position..position + output.len()]);
            return Ok(());
        }
        let remaining = self
            .len()
            .checked_sub(offset)
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        if remaining < output.len() as u64 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        // Grow read-ahead for a forward scan, but keep random/sparse prefix
        // reads small (for example, names before large inline textures).
        let adjacent = window.filled != 0
            && offset >= window.start
            && offset - window.start <= window.filled as u64 + Self::SMALL_READ as u64;
        let capacity = if adjacent {
            Self::CAPACITY
        } else {
            Self::SMALL_READ
        };
        let fill = usize::try_from(remaining.min(capacity as u64)).expect("file window fits usize");
        window.filled = 0;
        if window.bytes.len() < fill {
            let additional = fill - window.bytes.len();
            window
                .bytes
                .try_reserve_exact(additional)
                .map_err(io::Error::other)?;
            window.bytes.resize(fill, 0);
        }
        self.source
            .read_exact_at(offset, &mut window.bytes[..fill])?;
        window.start = offset;
        window.filled = fill;
        output.copy_from_slice(&window.bytes[..output.len()]);
        Ok(())
    }
}

impl FileSource {
    const fn new(file: File, length: u64) -> Self {
        Self {
            file: PositionalFile::new(file),
            length,
        }
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        self.file.read_exact_at(offset, output)
    }
}

struct MemorySource<B> {
    bytes: B,
}

impl<B: AsRef<[u8]> + Send + Sync> Source for MemorySource<B> {
    fn len(&self) -> u64 {
        u64::try_from(self.bytes.as_ref().len()).unwrap_or(u64::MAX)
    }

    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "memory source offset overflowed",
            )
        })?;
        let end = start.checked_add(output.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "memory read range overflowed")
        })?;
        let source = self.bytes.as_ref().get(start..end).ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "memory read exceeds source")
        })?;
        output.copy_from_slice(source);
        Ok(())
    }
}

struct SourceSegment {
    start: u64,
    region: Region,
}

/// Checks that a [`Source`] read of `output_length` bytes at `offset` stays
/// within `length`, returning the read's length. Errors name `context` (for
/// example "segmented read") and the `what` whose length was exceeded.
pub(crate) fn check_source_read(
    offset: u64,
    output_length: usize,
    length: u64,
    context: &str,
    what: &str,
) -> io::Result<u64> {
    let output_length = u64::try_from(output_length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "buffer length overflowed"))?;
    let end = offset.checked_add(output_length).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, format!("{context} overflowed"))
    })?;
    if end > length {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("{context} {offset}..{end} exceeds {what} length {length}"),
        ));
    }
    Ok(output_length)
}

struct SegmentedSource {
    segments: Vec<SourceSegment>,
    length: u64,
}

impl SegmentedSource {
    fn new(regions: Vec<Region>, maximum_length: u64) -> Result<Self> {
        let mut length = 0_u64;
        let mut segments = Vec::new();
        segments.try_reserve_exact(regions.len()).map_err(|error| {
            Error::invalid_data(format!("cannot allocate segmented source table: {error}"))
        })?;
        for region in regions {
            let start = length;
            length = length
                .checked_add(region.len())
                .ok_or_else(|| Error::invalid_data("segmented source length overflowed"))?;
            if length > maximum_length {
                return Err(Error::invalid_data(format!(
                    "segmented source is {length} bytes, exceeding limit {maximum_length}"
                )));
            }
            segments.push(SourceSegment { start, region });
        }
        Ok(Self { segments, length })
    }
}

impl Source for SegmentedSource {
    fn len(&self) -> u64 {
        self.length
    }

    fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> io::Result<()> {
        check_source_read(
            offset,
            output.len(),
            self.length,
            "segmented read",
            "source",
        )?;

        let mut source_offset = offset;
        let mut output_offset = 0_usize;
        while output_offset < output.len() {
            let segment_index = self
                .segments
                .partition_point(|segment| segment.start <= source_offset)
                .checked_sub(1)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "segmented read does not map to a source region",
                    )
                })?;
            let segment = &self.segments[segment_index];
            let region_offset = source_offset.checked_sub(segment.start).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "segment offset underflowed")
            })?;
            let available = segment
                .region
                .len()
                .checked_sub(region_offset)
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "segment offset exceeds its region",
                    )
                })?;
            if available == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "segmented read selected an empty region",
                ));
            }
            let output_remaining = output.len() - output_offset;
            let count =
                usize::try_from(available.min(u64::try_from(output_remaining).unwrap_or(u64::MAX)))
                    .expect("segment read length is bounded by the output slice");
            let output_end = output_offset.checked_add(count).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "output offset overflowed")
            })?;
            segment
                .region
                .read_exact_at_io(region_offset, &mut output[output_offset..output_end])?;
            source_offset = source_offset
                .checked_add(u64::try_from(count).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "read length overflowed")
                })?)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "source offset overflowed")
                })?;
            output_offset = output_end;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom};

    use super::Region;

    struct CountedSource {
        reads: std::sync::Mutex<Vec<(u64, usize)>>,
        length: u64,
    }

    impl super::Source for CountedSource {
        fn len(&self) -> u64 {
            self.length
        }
        fn read_exact_at(&self, offset: u64, output: &mut [u8]) -> std::io::Result<()> {
            self.reads.lock().unwrap().push((offset, output.len()));
            for (index, byte) in output.iter_mut().enumerate() {
                *byte = (offset + index as u64) as u8;
            }
            Ok(())
        }
    }

    #[test]
    fn metadata_read_ahead_reuses_reads_across_length_checks_and_stays_bounded() {
        let source = std::sync::Arc::new(CountedSource {
            reads: Default::default(),
            length: 128,
        });
        let region = Region::new(source.clone(), 11, 43).unwrap();
        let mut reader = crate::endian::EndianReader::new(
            region.buffered_cursor(16),
            crate::endian::Endian::Little,
        );
        for index in 0..20 {
            assert_eq!(reader.remaining().unwrap(), 43 - index);
            assert_eq!(reader.read_u8().unwrap(), (11 + index) as u8);
        }
        assert_eq!(
            source.reads.lock().unwrap().len(),
            2,
            "small fields must share source reads even when readers check their remaining length"
        );
        reader.set_position(15).unwrap();
        assert_eq!(
            reader.read_u32().unwrap(),
            u32::from_le_bytes([26, 27, 28, 29])
        );
        reader.set_position(42).unwrap();
        assert_eq!(reader.read_u8().unwrap(), 53);
        assert!(reader.read_u8().is_err());
        assert!(reader.set_position(44).is_err());
        assert!(
            source
                .reads
                .lock()
                .unwrap()
                .iter()
                .all(|&(offset, count)| offset >= 11 && offset + count as u64 <= 54)
        );
    }

    impl Region {
        fn read_ahead_bytes(&self, length: usize) -> Vec<u8> {
            let mut bytes = vec![0; length];
            self.read_ahead_cursor(length.max(1))
                .read_exact(&mut bytes)
                .unwrap();
            bytes
        }
    }

    #[test]
    fn file_read_ahead_is_shared_between_objects_bounded_and_bypassed_for_bulk_reads() {
        let source = std::sync::Arc::new(CountedSource {
            reads: Default::default(),
            length: 200_000,
        });
        let cached = std::sync::Arc::new(super::ReadAheadSource::new(source.clone()));
        let file = Region::new(cached.clone(), 0, source.length).unwrap();
        for start in [1024, 1080, 1200] {
            let object = file.subregion(start, 32).unwrap();
            let bytes = object.read_ahead_bytes(32);
            assert_eq!(
                bytes,
                (start..start + 32)
                    .map(|value| value as u8)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(
            source.reads.lock().unwrap().len(),
            1,
            "nearby object readers must reuse the same physical file read"
        );
        assert_eq!(
            source.reads.lock().unwrap()[0],
            (1024, 4096),
            "the first sparse prefix read should stay small"
        );
        file.subregion(5120, 32).unwrap().read_ahead_bytes(32);
        assert_eq!(
            source.reads.lock().unwrap().last(),
            Some(&(5120, 64 * 1024)),
            "a forward scan should grow read-ahead"
        );
        for start in [70_000, 0, 199_999] {
            let bytes = file.subregion(start, 1).unwrap().read_ahead_bytes(1);
            assert_eq!(bytes, vec![start as u8]);
        }
        let bulk = file.subregion(20_000, 5000).unwrap().read_ahead_bytes(5000);
        assert_eq!(bulk[0], 20_000u32 as u8);
        assert_eq!(
            source.reads.lock().unwrap().last(),
            Some(&(20_000, 5000)),
            "bulk reads must not fill the small-read cache"
        );
        assert!(cached.window.lock().unwrap().bytes.len() <= 64 * 1024);
        assert!(
            source
                .reads
                .lock()
                .unwrap()
                .iter()
                .all(|&(start, len)| start + len as u64 <= source.length)
        );
        assert!(file.subregion(199_999, 2).is_err());
        file.subregion(80_000, 32).unwrap().read_to_vec(32).unwrap();
        assert_eq!(
            source.reads.lock().unwrap().last(),
            Some(&(80_000, 32)),
            "ordinary payload readers must bypass scan read-ahead"
        );
    }

    #[test]
    fn independent_file_cursors_can_read_shared_windows_concurrently() {
        let source = std::sync::Arc::new(CountedSource {
            reads: Default::default(),
            length: 200_000,
        });
        let file = Region::new(
            std::sync::Arc::new(super::ReadAheadSource::new(source)),
            0,
            200_000,
        )
        .unwrap();
        std::thread::scope(|scope| {
            for thread in 0..4 {
                let file = file.clone();
                scope.spawn(move || {
                    for step in 0..300 {
                        let start = ((step * 1777 + thread * 73) % 190_000) as u64;
                        let bytes = file.subregion(start, 100).unwrap().read_ahead_bytes(100);
                        assert_eq!(
                            bytes,
                            (start..start + 100)
                                .map(|value| value as u8)
                                .collect::<Vec<_>>()
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn buffered_cursors_match_direct_reads_across_seeks_windows_and_bulk_reads() {
        let region = Region::from_bytes((0..128).collect::<Vec<u8>>())
            .subregion(9, 71)
            .unwrap();
        for capacity in [1, 16, 64 * 1024] {
            let mut cached = region.buffered_cursor(capacity);
            let mut direct = region.cursor();
            for offset in [0, 7, 16, 3, 69, 71, 25] {
                for length in [1, 3, 17, 72] {
                    cached.seek(SeekFrom::Start(offset)).unwrap();
                    direct.seek(SeekFrom::Start(offset)).unwrap();
                    let mut a = vec![0; length];
                    let mut b = vec![0; length];
                    assert_eq!(
                        cached.read_exact(&mut a).is_ok(),
                        direct.read_exact(&mut b).is_ok()
                    );
                    assert_eq!(a, b);
                    assert_eq!(cached.position(), direct.position());
                }
            }
        }
    }

    #[test]
    fn subregions_and_cursors_are_strictly_bounded() {
        let root = Region::from_bytes(Vec::from(&b"0123456789"[..]));
        let region = root.subregion(2, 5).unwrap();
        let mut cursor = region.cursor();
        let mut bytes = Vec::new();
        cursor.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"23456");
        assert!(cursor.seek(SeekFrom::Start(6)).is_err());
        assert!(root.subregion(8, 3).is_err());
    }

    #[test]
    fn owned_vector_regions_read_like_shared_byte_regions() {
        let bytes = b"0123456789".to_vec();
        let owned = Region::from_vec(bytes.clone());
        let shared = Region::from_bytes(bytes);
        assert_eq!(owned.len(), shared.len());
        assert_eq!(
            owned.read_to_vec(10).unwrap(),
            shared.read_to_vec(10).unwrap()
        );
        let owned_tail = owned.subregion(4, 6).unwrap();
        let shared_tail = shared.subregion(4, 6).unwrap();
        assert_eq!(
            owned_tail.read_to_vec(6).unwrap(),
            shared_tail.read_to_vec(6).unwrap()
        );
        assert!(owned.subregion(8, 3).is_err());
        assert!(owned.read_to_vec(9).is_err());
    }

    #[test]
    fn cloned_cursors_have_independent_positions() {
        let region = Region::from_bytes(Vec::from(&b"abcdef"[..]));
        let mut first = region.cursor();
        let mut second = region.cursor();
        let mut byte = [0_u8; 1];

        first.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"a");
        second.seek(SeekFrom::Start(3)).unwrap();
        second.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"d");
        first.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"b");
    }

    #[test]
    fn copies_and_materializes_with_limits() {
        let region = Region::from_bytes(Vec::from(&b"payload"[..]));
        let mut output = Vec::new();
        assert_eq!(region.copy_range(1, 3, &mut output).unwrap(), 3);
        assert_eq!(output, b"ayl");
        assert_eq!(region.read_to_vec(7).unwrap(), b"payload");
        assert!(region.read_to_vec(6).is_err());
    }

    #[test]
    fn concatenated_regions_read_lazily_across_segment_boundaries() {
        let region = Region::concatenate(
            vec![
                Region::from_bytes(Vec::from(&b"abc"[..])),
                Region::from_bytes(Vec::<u8>::new()),
                Region::from_bytes(Vec::from(&b"defg"[..])),
            ],
            7,
        )
        .unwrap();

        assert_eq!(region.len(), 7);
        let mut bytes = [0_u8; 5];
        region.read_exact_at(1, &mut bytes).unwrap();
        assert_eq!(&bytes, b"bcdef");
        assert_eq!(
            region.subregion(2, 4).unwrap().read_to_vec(4).unwrap(),
            b"cdef"
        );
        assert!(Region::concatenate(vec![region], 6).is_err());
    }
}
