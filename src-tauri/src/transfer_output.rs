//! 有界 UTF-8 -> 目标编码写入，以及同目录、无覆盖的导出发布。
//! 编码错误绝不替换字符；未 finish 的输出只能作为临时文件清理。
use std::fs::{File, Metadata, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Component, Path, PathBuf};
use anyhow::{anyhow, bail, Context, Result};
use encoding_rs::{Decoder, DecoderResult, Encoder, EncoderResult};

pub(super) const BUFFER_BYTES: usize = 64 * 1024;
enum OutputEncoding { Utf8, Latin1, Legacy(Encoder) }

pub(super) struct EncodedWriter<W: Write> {
    inner: W,
    decoder: Decoder,
    encoding: OutputEncoding,
    text: Vec<u8>,
    bytes: Vec<u8>,
    failed: bool,
    finished: bool,
    written: u64,
}
impl<W: Write> EncodedWriter<W> {
    pub(super) fn new(inner: W, label: &str) -> Result<Self> {
        let encoding = match label.to_ascii_lowercase().as_str() {
            "" | "utf-8" | "utf8" => OutputEncoding::Utf8,
            "latin1" | "iso-8859-1" => OutputEncoding::Latin1,
            "gbk" | "gb2312" => OutputEncoding::Legacy(encoding_rs::GBK.new_encoder()),
            "gb18030" => OutputEncoding::Legacy(encoding_rs::GB18030.new_encoder()),
            "big5" => OutputEncoding::Legacy(encoding_rs::BIG5.new_encoder()),
            _ => bail!("不支持的编码: {label}"),
        };
        Ok(Self { inner, decoder: encoding_rs::UTF_8.new_decoder_without_bom_handling(), encoding,
            text: vec![0; BUFFER_BYTES], bytes: vec![0; BUFFER_BYTES], failed: false, finished: false, written: 0 })
    }
    fn invalid(message: &str) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, message) }
    fn encode_text(&mut self, length: usize, last: bool) -> io::Result<()> {
        let text = std::str::from_utf8(&self.text[..length]).map_err(|_| Self::invalid("导出编码器产生了无效 UTF-8"))?;
        match &mut self.encoding {
            OutputEncoding::Utf8 => { self.inner.write_all(text.as_bytes())?; self.written += length as u64; }
            OutputEncoding::Latin1 => {
                let mut count = 0;
                for c in text.chars() {
                    let b = u8::try_from(c as u32).map_err(|_| Self::invalid("内容不能以 ISO-8859-1 无损保存"))?;
                    self.bytes[count] = b; count += 1;
                    if count == self.bytes.len() { self.inner.write_all(&self.bytes[..count])?; self.written += count as u64; count = 0; }
                }
                self.inner.write_all(&self.bytes[..count])?; self.written += count as u64;
            }
            OutputEncoding::Legacy(encoder) => {
                let mut consumed = 0;
                loop {
                    let (result, read, written) = encoder.encode_from_utf8_without_replacement(&text[consumed..], &mut self.bytes, last);
                    self.inner.write_all(&self.bytes[..written])?; self.written += written as u64; consumed += read;
                    match result {
                        EncoderResult::InputEmpty => break,
                        EncoderResult::Unmappable(_) => return Err(Self::invalid("内容不能以所选编码无损保存；没有发布导出文件")),
                        EncoderResult::OutputFull if read == 0 && written == 0 => return Err(Self::invalid("导出编码器无法取得进展")),
                        EncoderResult::OutputFull => {},
                    }
                }
            }
        }
        Ok(())
    }
    fn push(&mut self, input: &[u8], last: bool) -> io::Result<()> {
        let mut consumed = 0;
        loop {
            let (result, read, written) = self.decoder.decode_to_utf8_without_replacement(&input[consumed..], &mut self.text, last);
            consumed += read;
            // 最终 decoder InputEmpty 才允许 encoder 完成；OutputFull 必须继续消费原输入。
            self.encode_text(written, last && result == DecoderResult::InputEmpty)?;
            match result {
                DecoderResult::InputEmpty => return Ok(()),
                DecoderResult::Malformed(_, _) => return Err(Self::invalid("导出输入包含无效或截断的 UTF-8")),
                DecoderResult::OutputFull if read == 0 && written == 0 => return Err(Self::invalid("导出解码器无法取得进展")),
                DecoderResult::OutputFull => {},
            }
        }
    }
    pub(super) fn finish(mut self) -> io::Result<W> {
        if self.failed { return Err(Self::invalid("导出编码器已经失败，不能发布")); }
        if let Err(error) = self.push(&[], true) { self.failed = true; return Err(error); }
        self.finished = true; self.inner.flush()?; Ok(self.inner)
    }
    pub(super) fn bytes_written(&self) -> u64 { self.written }
}
impl<W: Write> Write for EncodedWriter<W> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.is_empty() { return Ok(0); }
        if self.failed || self.finished { return Err(Self::invalid("导出编码器已结束或失败")); }
        if let Err(error) = self.push(input, false) { self.failed = true; return Err(error); }
        Ok(input.len())
    }
    // flush 不视作 EOF，允许多字节字符跨 write/flush；finish 才严格核对尾部。
    fn flush(&mut self) -> io::Result<()> {
        if self.failed { return Err(Self::invalid("导出编码器已经失败")); }
        self.inner.flush()
    }
}
pub(super) type FileEncoder = EncodedWriter<BufWriter<File>>;

pub(super) fn is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() { return true; }
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 { return true; }
    }
    false
}
pub(super) fn checked_absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir()?.join(path) };
    if path.components().any(|c| matches!(c, Component::ParentDir)) { bail!("文件路径不能含父目录跳转"); }
    let mut part = PathBuf::new();
    for component in path.components() {
        part.push(component.as_os_str());
        match std::fs::symlink_metadata(&part) {
            Ok(meta) if is_link_or_reparse(&meta) => bail!("文件路径不能经过符号链接或重解析点"),
            Ok(_) => {},
            // NotFound 表示该层级尚不存在（目录待创建 / 文件尚未生成），不是链接风险；
            // 继续向上验证其余层级。Windows 对不存在路径也可能返回 ERROR_INVALID_FUNCTION(1)。
            Err(error) if matches!(error.kind(), io::ErrorKind::NotFound) => {},
            Err(error) if error.raw_os_error() == Some(1) && part != path => {},
            Err(error) => return Err(error).context("无法验证文件路径"),
        }
    }
    Ok(path)
}

#[cfg(unix)]
pub(super) fn identity(file: &File) -> Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?; Ok((meta.dev(), meta.ino()))
}
#[cfg(windows)]
pub(super) fn identity(file: &File) -> Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)] struct Information { attributes:u32, creation:[u32;2], access:[u32;2], write:[u32;2], volume:u32,
        size_high:u32, size_low:u32, links:u32, index_high:u32, index_low:u32 }
    #[link(name="kernel32")] unsafe extern "system" { fn GetFileInformationByHandle(handle:*mut std::ffi::c_void, info:*mut Information)->i32; }
    let mut info = std::mem::MaybeUninit::<Information>::uninit();
    // SAFETY: File 的有效 HANDLE 在调用期间存活，系统只写入完整 repr(C) 信息结构。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 { return Err(io::Error::last_os_error().into()); }
    let info = unsafe { info.assume_init() }; Ok((info.volume as u64, ((info.index_high as u64)<<32)|info.index_low as u64))
}
#[cfg(not(any(unix,windows)))]
pub(super) fn identity(_file: &File) -> Result<(u64,u64)> { bail!("当前平台不能验证文件物理身份") }
fn open_directory(path:&Path) -> Result<File> {
    let mut options=OpenOptions::new(); options.read(true);
    #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.custom_flags(0x02000000).share_mode(1|2); }
    options.open(path).context("无法固定导出目录")
}

/// stage 始终由本对象拥有。hard_link 在所有支持平台均 create-only，绝不 rename 覆盖。
/// 不支持硬链接/目录身份验证的文件系统会明确失败，禁止降级为先删或覆盖目标。
pub(super) struct StagedOutput {
    target: PathBuf,
    stage: PathBuf,
    directory: File,
    directory_id: (u64,u64),
    file: Option<File>,
    file_id: (u64,u64),
    published: bool,
}
impl StagedOutput {
    pub(super) fn new(target: &str) -> Result<Self> {
        let target=checked_absolute(Path::new(target))?;
        match std::fs::symlink_metadata(&target) {
            Ok(_) => bail!("导出目标已存在；请选择新文件，未覆盖任何内容"),
            Err(error) if error.kind()==io::ErrorKind::NotFound => {},
            Err(error) => return Err(error).context("无法检查导出目标"),
        }
        let parent=target.parent().ok_or_else(||anyhow!("导出目标缺少目录"))?;
        let parent=std::fs::canonicalize(parent)?;
        let name=target.file_name().ok_or_else(||anyhow!("导出目标缺少文件名"))?;
        let target=parent.join(name);
        let directory=open_directory(&parent)?; let directory_id=identity(&directory)?;
        let stage=parent.join(format!(".navkit-export-{}.tmp",uuid::Uuid::new_v4()));
        let mut options=OpenOptions::new(); options.read(true).write(true).create_new(true);
        #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        // 阻止外部写入/删除 stage。创建目标硬链接不删除源链接。
        #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
        let file=options.open(&stage)?; let file_id=identity(&file)?;
        Ok(Self {target,stage,directory,directory_id,file:Some(file),file_id,published:false})
    }
    pub(super) fn file_clone(&self) -> Result<File> {
        Ok(self.file.as_ref().ok_or_else(||anyhow!("stage 已关闭"))?.try_clone()?)
    }
    pub(super) fn encoder(&self, label:&str) -> Result<FileEncoder> {
        let file=self.file.as_ref().ok_or_else(||anyhow!("导出 stage 已关闭"))?.try_clone()?;
        EncodedWriter::new(BufWriter::with_capacity(BUFFER_BYTES,file),label)
    }
    fn stage_is_owned(&self)->bool { File::open(&self.stage).ok().and_then(|f|identity(&f).ok())==Some(self.file_id) }
    pub(super) fn publish(mut self, check: impl FnOnce()->Result<()>) -> Result<()> {
        self.file.as_ref().ok_or_else(||anyhow!("导出 stage 已关闭"))?.sync_all()?;
        let parent=self.target.parent().unwrap(); let current=open_directory(parent)?;
        if identity(&current)?!=self.directory_id || !self.stage_is_owned() { bail!("导出目录或临时文件已经变化，没有发布"); }
        check()?;
        std::fs::hard_link(&self.stage,&self.target).context("无法无覆盖发布导出文件（目标可能已存在或文件系统不支持硬链接）")?;
        self.published=true;
        let published_id = File::open(&self.target).map_err(|error|anyhow!("TRANSFER_EFFECTS_UNKNOWN: 导出目标已建立，但身份读回失败，需要核对目标文件: {error}"))?;
        let published_id = identity(&published_id).map_err(|error|anyhow!("TRANSFER_EFFECTS_UNKNOWN: 导出目标已建立，但物理身份核验失败，需要核对目标文件: {error}"))?;
        if published_id!=self.file_id { bail!("TRANSFER_EFFECTS_UNKNOWN: 导出发布物理身份不匹配；需要核对目标文件"); }
        // 目标已提交。此后不再把迟到的取消误报成“文件未写入”。
        #[cfg(unix)] self.directory.sync_all().context("TRANSFER_EFFECTS_UNKNOWN: 导出文件已发布，但目录同步失败；需要核对目标文件")?;
        self.file.take();
        if self.stage_is_owned() { std::fs::remove_file(&self.stage).context("TRANSFER_EFFECTS_UNKNOWN: 导出已发布，但临时链接清理失败；不能宣称没有写入目标")?; }
        Ok(())
    }
}
impl Drop for StagedOutput {
    fn drop(&mut self) {
        let owned=self.stage_is_owned(); self.file.take();
        if owned { let _=std::fs::remove_file(&self.stage); }
    }
}

#[cfg(test)] mod tests {
    use super::*;
    use std::io::Read;
    use crate::transfer_stream::DecodeReader;
    #[test] fn encoder_short_utf8_writes_and_flush_preserve_bom_nul_and_latin1() {
        let expected="\u{0}\u{81}éÿ";
        for chunk in 1..=5 { let mut writer=EncodedWriter::new(Vec::new(),"latin1").unwrap();
            for bytes in expected.as_bytes().chunks(chunk) {writer.write_all(bytes).unwrap();writer.flush().unwrap();}
            assert_eq!(writer.finish().unwrap(),vec![0,0x81,0xe9,0xff]); }
        let mut utf8=EncodedWriter::new(Vec::new(),"utf-8").unwrap();utf8.write_all("\u{feff}中文\0".as_bytes()).unwrap();
        assert_eq!(utf8.finish().unwrap(),"\u{feff}中文\0".as_bytes());
    }
    #[test] fn gbk_output_does_not_silently_expand_to_gb18030() {
        for label in ["gbk", "gb2312"] {
            let mut writer = EncodedWriter::new(Vec::new(), label).unwrap();
            writer.write_all("中文".as_bytes()).unwrap();
            let bytes = writer.finish().unwrap();
            let mut reader = DecodeReader::new(bytes.as_slice(), label).unwrap();
            let mut decoded = String::new(); reader.read_to_string(&mut decoded).unwrap(); assert_eq!(decoded, "中文");
            let mut writer = EncodedWriter::new(Vec::new(), label).unwrap();
            assert!(writer.write_all("😀".as_bytes()).is_err()); assert!(writer.finish().is_err());
        }
    }
    #[test] fn legacy_encoder_roundtrips_at_every_unicode_boundary() {
        // WHATWG Big5 的编码集合小于解码集合：é 位于仅解码的指针 1144（88 6D）。
        // 正例保留 1/3/4 字节 UTF-8 与 NUL；同时核对标准 Big5 字节，避免双向错误互相掩盖。
        for (label,text) in [("gb18030","中文\u{80}\u{1f600}\0"),("big5","中文€\u{27607}\0")] {
            for chunk in 1..=7 {let mut writer=EncodedWriter::new(Vec::new(),label).unwrap();
                for bytes in text.as_bytes().chunks(chunk){writer.write_all(bytes).unwrap();}
                let bytes=writer.finish().unwrap();
                if label=="big5" {assert_eq!(bytes,b"\xA4\xA4\xA4\xE5\xA3\xE1\xC8\xA4\0");}
                let mut reader=DecodeReader::new(bytes.as_slice(),label).unwrap();
                let mut decoded=String::new();reader.read_to_string(&mut decoded).unwrap();assert_eq!(decoded,text);}
        }
    }
    #[test] fn encoder_unmappable_and_truncated_input_are_sticky_failures() {
        // 88 6D 可严格解码为 UTF-8 C3 A9，但 Big5 Encoder 必须拒绝反向的 U+00E9。
        let mut reference=encoding_rs::BIG5.new_encoder();let mut output=[0u8;8];
        let (result,read,written)=reference.encode_from_utf8_without_replacement("é",&mut output,true);
        assert_eq!(result,EncoderResult::Unmappable('é'));assert_eq!((read,written),(2,0));
        let mut reader=DecodeReader::new(&b"\x88\x6D"[..],"big5").unwrap();let mut decoded=String::new();
        reader.read_to_string(&mut decoded).unwrap();assert_eq!(decoded.as_bytes(),b"\xC3\xA9");
        for (label,bytes) in [("latin1","€".as_bytes()),("big5","😀".as_bytes()),("big5","é".as_bytes()),("utf-8",&[0xff][..])] {
            let mut writer=EncodedWriter::new(Vec::new(),label).unwrap();assert!(writer.write_all(bytes).is_err());
            assert!(writer.write_all(b"retry").is_err());assert!(writer.finish().is_err());
        }
        let mut writer=EncodedWriter::new(Vec::new(),"gb18030").unwrap();writer.write_all(&[0xe4,0xb8]).unwrap();assert!(writer.finish().is_err());
    }
    #[derive(Default)] struct CountWriter {bytes:u64,max_write:usize}
    impl Write for CountWriter {fn write(&mut self,b:&[u8])->io::Result<usize>{let n=b.len().min(17);self.max_write=self.max_write.max(b.len());self.bytes+=n as u64;Ok(n)}fn flush(&mut self)->io::Result<()>{Ok(())}}
    #[test] fn generated_large_encoder_stream_has_fixed_buffers_and_backpressure() {
        for (label,part,output_len) in [("latin1","éABC",4),("gb18030","中😀",6),("big5","中文",4)] {
            let mut writer=EncodedWriter::new(CountWriter::default(),label).unwrap();
            let pointers=(writer.text.as_ptr(),writer.bytes.as_ptr());
            for _ in 0..500_000 {writer.write_all(part.as_bytes()).unwrap();assert_eq!((writer.text.len(),writer.bytes.len()),(BUFFER_BYTES,BUFFER_BYTES));assert_eq!((writer.text.as_ptr(),writer.bytes.as_ptr()),pointers);}
            let out=writer.finish().unwrap();assert_eq!(out.bytes,500_000*output_len);assert!(out.max_write<=BUFFER_BYTES);
        }
    }
    struct Directory(PathBuf);
    impl Directory {fn new()->Self{let p=std::env::temp_dir().join(format!("navkit-output-test-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&p).unwrap();Self(p)}}
    impl Drop for Directory {fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
    #[test] fn publisher_never_clobbers_early_or_late_targets() {
        let dir=Directory::new();let target=dir.0.join("out.csv");std::fs::write(&target,b"original").unwrap();
        assert!(StagedOutput::new(target.to_str().unwrap()).is_err());assert_eq!(std::fs::read(&target).unwrap(),b"original");
        std::fs::remove_file(&target).unwrap();let stage=StagedOutput::new(target.to_str().unwrap()).unwrap();
        let mut writer=stage.encoder("latin1").unwrap();writer.write_all("é".as_bytes()).unwrap();writer.finish().unwrap();
        std::fs::write(&target,b"late").unwrap();assert!(stage.publish(||Ok(())).is_err());assert_eq!(std::fs::read(&target).unwrap(),b"late");
        assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(),1);
    }
    #[test] fn publisher_cancel_and_encoding_failure_leave_no_partial_output() {
        let dir=Directory::new();let target=dir.0.join("out.json");
        {let stage=StagedOutput::new(target.to_str().unwrap()).unwrap();let mut w=stage.encoder("latin1").unwrap();w.write_all(b"[").unwrap();assert!(w.write_all("😀".as_bytes()).is_err());drop(w);drop(stage);}
        assert!(!target.exists());assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(),0);
        let stage=StagedOutput::new(target.to_str().unwrap()).unwrap();stage.encoder("utf8").unwrap().finish().unwrap();
        assert!(stage.publish(||bail!("cancelled before publish")).is_err());assert!(!target.exists());
    }
    #[test] fn publisher_complete_file_and_no_partial_stage_remain() {
        let dir=Directory::new();let target=dir.0.join("complete.csv");let stage=StagedOutput::new(target.to_str().unwrap()).unwrap();
        let mut writer=stage.encoder("gb18030").unwrap();writer.write_all("列\r\n中文\r\n".as_bytes()).unwrap();writer.finish().unwrap();stage.publish(||Ok(())).unwrap();
        let mut reader=DecodeReader::new(File::open(&target).unwrap(),"gb18030").unwrap();let mut content=String::new();reader.read_to_string(&mut content).unwrap();assert_eq!(content,"列\r\n中文\r\n");assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(),1);
    }
    #[cfg(unix)] #[test] fn publisher_rejects_symlink_parent_and_target() {
        use std::os::unix::fs::symlink;
        let dir=Directory::new();let link=dir.0.join("link");symlink(&dir.0,&link).unwrap();assert!(StagedOutput::new(link.join("out.csv").to_str().unwrap()).is_err());
        let target=dir.0.join("target.csv");symlink(dir.0.join("absent"),&target).unwrap();assert!(StagedOutput::new(target.to_str().unwrap()).is_err());
    }
}
