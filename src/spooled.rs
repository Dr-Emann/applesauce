use crate::checked_add_signed;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::{io, mem};

pub enum SpooledData<W> {
    Memory(Cursor<Vec<u8>>),
    File(W),
}

pub struct SpooledFile<W, F> {
    data: SpooledData<W>,
    create: Option<F>,
}

impl<W: Write + Seek, F: FnOnce() -> W> SpooledFile<W, F> {
    pub fn new(capacity: usize, f: F) -> Self {
        Self {
            data: SpooledData::Memory(Cursor::new(Vec::with_capacity(capacity))),
            create: Some(f),
        }
    }

    pub fn roll(&mut self) -> io::Result<()> {
        if let SpooledData::Memory(m) = &mut self.data {
            let create = self.create.take().unwrap();
            let mut file = create();
            let data = mem::take(m);
            let offset = data.position();
            file.write_all(&data.into_inner())?;
            file.seek(SeekFrom::Start(offset))?;
            self.data = SpooledData::File(file);
        }
        Ok(())
    }

    pub fn finish(self) -> SpooledData<W> {
        self.data
    }
}

impl<W: Write + Seek, F: FnOnce() -> W> Write for SpooledFile<W, F> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let SpooledData::Memory(m) = &self.data {
            if (m.position() as usize).saturating_add(buf.len()) > m.get_ref().capacity() {
                self.roll()?;
            }
        }

        match &mut self.data {
            SpooledData::Memory(m) => m.write(buf),
            SpooledData::File(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if let SpooledData::File(w) = &mut self.data {
            w.flush()?;
        }
        Ok(())
    }
}

impl<W: Write + Seek, F: FnOnce() -> W> Seek for SpooledFile<W, F> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if let SpooledData::Memory(m) = &mut self.data {
            let end_pos = match pos {
                SeekFrom::Start(i) => i,
                SeekFrom::End(i) => checked_add_signed(m.get_ref().len() as u64, i)
                    .ok_or(io::ErrorKind::InvalidInput)?,
                SeekFrom::Current(i) => {
                    checked_add_signed(m.position(), i).ok_or(io::ErrorKind::InvalidInput)?
                }
            };
            if end_pos > m.get_ref().capacity() as u64 {
                self.roll()?;
            } else {
                m.set_position(end_pos);
                return Ok(end_pos);
            }
        }

        let w = match &mut self.data {
            SpooledData::Memory(_) => unreachable!("roll must have been called"),
            SpooledData::File(w) => w,
        };

        w.seek(pos)
    }
}
