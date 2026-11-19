//! Streaming Matroska muxer for OMT's original VMX1 video and float PCM.
use std::io::{self, Seek, SeekFrom, Write};

pub struct Mux<W> {
    output: W,
    cluster: Option<u64>,
    duration_offset: u64,
    duration_ms: u64,
    frame_ms: u64,
}

fn size(out: &mut impl Write, n: usize) -> io::Result<()> {
    let n = n as u64;
    let width = (1..=8).find(|width| n < (1u64 << (7 * width)) - 1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Matroska element too large"))?;
    out.write_all(&(n | (1 << (7 * width))).to_be_bytes()[8 - width..])
}

fn element(out: &mut impl Write, id: &[u8], data: &[u8]) -> io::Result<()> {
    out.write_all(id)?;
    size(out, data.len())?;
    out.write_all(data)
}

fn integer(out: &mut Vec<u8>, id: &[u8], n: u64) -> io::Result<()> {
    let bytes = n.to_be_bytes();
    let offset = (n.leading_zeros() as usize / 8).min(7);
    element(out, id, &bytes[offset..])
}

impl<W: Write + Seek> Mux<W> {
    pub fn new(mut output: W, width: u32, height: u32, fps: (u32, u32), audio: Option<(u32, u32)>) -> io::Result<Self> {
        let mut header = Vec::new();
        integer(&mut header, &[0x42, 0x86], 1)?;
        integer(&mut header, &[0x42, 0xf7], 1)?;
        integer(&mut header, &[0x42, 0xf2], 4)?;
        integer(&mut header, &[0x42, 0xf3], 8)?;
        element(&mut header, &[0x42, 0x82], b"matroska")?;
        integer(&mut header, &[0x42, 0x87], 4)?;
        integer(&mut header, &[0x42, 0x85], 2)?;
        element(&mut output, &[0x1a, 0x45, 0xdf, 0xa3], &header)?;
        output.write_all(&[0x18, 0x53, 0x80, 0x67, 0xff])?;
        let mut info = Vec::new();
        integer(&mut info, &[0x2a, 0xd7, 0xb1], 1_000_000)?;
        element(&mut info, &[0x4d, 0x80], b"LiveMix")?;
        element(&mut info, &[0x57, 0x41], b"LiveMix")?;
        element(&mut info, &[0x44, 0x89], &0f64.to_be_bytes())?;
        element(&mut output, &[0x15, 0x49, 0xa9, 0x66], &info)?;
        let duration_offset = output.stream_position()? - 8;

        let mut video = Vec::new();
        integer(&mut video, &[0xb0], width.into())?;
        integer(&mut video, &[0xba], height.into())?;
        let mut track = Vec::new();
        integer(&mut track, &[0xd7], 1)?;
        integer(&mut track, &[0x73, 0xc5], 1)?;
        integer(&mut track, &[0x83], 1)?;
        integer(&mut track, &[0x23, 0xe3, 0x83], 1_000_000_000 * u64::from(fps.1) / u64::from(fps.0))?;
        element(&mut track, &[0x86], b"V_MS/VFW/FOURCC")?;
        let mut bitmap = [0u8; 40];
        bitmap[..4].copy_from_slice(&40u32.to_le_bytes());
        bitmap[4..8].copy_from_slice(&width.to_le_bytes());
        bitmap[8..12].copy_from_slice(&height.to_le_bytes());
        bitmap[12..14].copy_from_slice(&1u16.to_le_bytes());
        bitmap[14..16].copy_from_slice(&24u16.to_le_bytes());
        bitmap[16..20].copy_from_slice(b"VMX1");
        element(&mut track, &[0x63, 0xa2], &bitmap)?;
        element(&mut track, &[0xe0], &video)?;
        let mut tracks = Vec::new();
        element(&mut tracks, &[0xae], &track)?;
        if let Some((rate, channels)) = audio {
            track.clear();
            integer(&mut track, &[0xd7], 2)?;
            integer(&mut track, &[0x73, 0xc5], 2)?;
            integer(&mut track, &[0x83], 2)?;
            element(&mut track, &[0x86], b"A_PCM/FLOAT/IEEE")?;
            let mut settings = Vec::new();
            element(&mut settings, &[0xb5], &f64::from(rate).to_be_bytes())?;
            integer(&mut settings, &[0x9f], channels.into())?;
            integer(&mut settings, &[0x62, 0x64], 32)?;
            element(&mut track, &[0xe1], &settings)?;
            element(&mut tracks, &[0xae], &track)?;
        }
        element(&mut output, &[0x16, 0x54, 0xae, 0x6b], &tracks)?;
        Ok(Self { output, cluster: None, duration_offset, duration_ms: 0,
            frame_ms: (1000 * u64::from(fps.1)).div_ceil(u64::from(fps.0)) })
    }

    pub fn packet(&mut self, track: u8, millis: u64, bytes: &[u8]) -> io::Result<()> {
        if track == 1 { self.duration_ms = self.duration_ms.max(millis.saturating_add(self.frame_ms)); }
        if self.cluster.is_none_or(|base| millis >= base.saturating_add(1000)) {
            let mut time = Vec::new();
            integer(&mut time, &[0xe7], millis)?;
            self.output.write_all(&[0x1f, 0x43, 0xb6, 0x75, 0xff])?;
            self.output.write_all(&time)?;
            self.cluster = Some(millis);
        }
        let offset = millis as i128 - i128::from(self.cluster.unwrap());
        let offset = i16::try_from(offset).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "OMT timestamps discontinuous"))?;
        self.output.write_all(&[0xa3])?;
        size(&mut self.output, bytes.len() + 4)?;
        self.output.write_all(&[0x80 | track, offset.to_be_bytes()[0], offset.to_be_bytes()[1], 0x80])?;
        self.output.write_all(bytes)
    }

    pub fn checkpoint(&mut self) -> io::Result<()> {
        let end = self.output.stream_position()?;
        self.output.seek(SeekFrom::Start(self.duration_offset))?;
        self.output.write_all(&(self.duration_ms as f64).to_be_bytes())?;
        self.output.seek(SeekFrom::Start(end))?;
        self.output.flush()
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.output.seek(SeekFrom::Start(self.duration_offset))?;
        self.output.write_all(&(self.duration_ms as f64).to_be_bytes())?;
        self.output.flush()
    }
}
