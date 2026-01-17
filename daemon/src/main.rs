use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory,
    ReplyEntry, ReplyOpen, ReplyData, Request,
};
use libc::{ENOENT, O_RDONLY};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::io::Read;

const TTL: Duration = Duration::from_secs(1);

struct PassthroughFS {
    source: PathBuf,
}

impl PassthroughFS {
    fn new(source: PathBuf) -> Self {
        PassthroughFS { source }
    }

    fn real_path(&self, ino: u64) -> PathBuf {
        // Para el POC, usamos un mapeo simple: ino 1 = root
        // En producción esto sería una tabla de inodos
        if ino == 1 {
            self.source.clone()
        } else {
            // Por ahora, mapeo temporal para archivos
            self.source.clone()
        }
    }

    fn get_file_attr(&self, path: &Path) -> Result<FileAttr, i32> {
        match fs::metadata(path) {
            Ok(metadata) => {
                let kind = if metadata.is_dir() {
                    FileType::Directory
                } else if metadata.is_file() {
                    FileType::RegularFile
                } else if metadata.is_symlink() {
                    FileType::Symlink
                } else {
                    FileType::RegularFile
                };

                let atime = metadata.accessed()
                    .unwrap_or(UNIX_EPOCH);
                let mtime = metadata.modified()
                    .unwrap_or(UNIX_EPOCH);
                let ctime = SystemTime::now();

                Ok(FileAttr {
                    ino: metadata.ino(),
                    size: metadata.len(),
                    blocks: metadata.blocks(),
                    atime,
                    mtime,
                    ctime,
                    crtime: SystemTime::UNIX_EPOCH,
                    kind,
                    perm: metadata.mode() as u16,
                    nlink: metadata.nlink() as u32,
                    uid: metadata.uid(),
                    gid: metadata.gid(),
                    rdev: metadata.rdev() as u32,
                    flags: 0,
                    blksize: metadata.blksize() as u32,
                })
            }
            Err(_) => Err(ENOENT),
        }
    }
}

impl Filesystem for PassthroughFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        println!("lookup: parent={}, name={:?}", parent, name);
        
        let parent_path = self.real_path(parent);
        let file_path = parent_path.join(name);
        
        match self.get_file_attr(&file_path) {
            Ok(attr) => {
                reply.entry(&TTL, &attr, 0);
            }
            Err(e) => {
                reply.error(e);
            }
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        println!("getattr: ino={}", ino);
        
        let path = self.real_path(ino);
        
        match self.get_file_attr(&path) {
            Ok(attr) => {
                reply.attr(&TTL, &attr);
            }
            Err(e) => {
                reply.error(e);
            }
        }
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        println!("read: ino={}, offset={}, size={}", ino, offset, size);
        
        let path = self.real_path(ino);
        
        match fs::File::open(&path) {
            Ok(mut file) => {
                use std::io::Seek;
                
                if let Err(_) = file.seek(std::io::SeekFrom::Start(offset as u64)) {
                    reply.error(libc::EIO);
                    return;
                }
                
                let mut buffer = vec![0; size as usize];
                match file.read(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        reply.data(&buffer);
                    }
                    Err(_) => {
                        reply.error(libc::EIO);
                    }
                }
            }
            Err(_) => {
                reply.error(ENOENT);
            }
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        println!("readdir: ino={}, offset={}", ino, offset);
        
        let path = self.real_path(ino);
        
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut index = 0;
        
        // Añadir . y ..
        if offset == 0 {
            let _ = reply.add(ino, index, FileType::Directory, ".");
            index += 1;
        }
        if offset <= 1 {
            let _ = reply.add(ino, index, FileType::Directory, "..");
            index += 1;
        }

        // Añadir archivos reales
        for (i, entry) in entries.enumerate().skip(offset.max(2) as usize - 2) {
            if let Ok(entry) = entry {
                if let Ok(metadata) = entry.metadata() {
                    let kind = if metadata.is_dir() {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    };
                    
                    let full = reply.add(
                        metadata.ino(),
                        (i + 2) as i64,
                        kind,
                        entry.file_name(),
                    );
                    
                    if full {
                        break;
                    }
                }
            }
        }
        
        reply.ok();
    }

    fn open(&mut self, _req: &Request, ino: u64, flags: i32, reply: ReplyOpen) {
        println!("open: ino={}, flags={}", ino, flags);
        
        // Por ahora, permitir cualquier apertura de solo lectura
        if flags & libc::O_ACCMODE == O_RDONLY {
            reply.opened(0, 0);
        } else {
            reply.error(libc::EACCES);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() != 3 {
        eprintln!("Usage: {} <source> <mountpoint>", args[0]);
        eprintln!("Example: {} /mnt/kriptofs-storage $HOME/Protected", args[0]);
        std::process::exit(1);
    }

    let source = PathBuf::from(&args[1]);
    let mountpoint = &args[2];

    if !source.exists() {
        eprintln!("Error: Source directory does not exist: {:?}", source);
        std::process::exit(1);
    }

    println!("=================================");
    println!("KriptoFS POC v0.2 - Passthrough");
    println!("=================================");
    println!("Source: {:?}", source);
    println!("Mountpoint: {}", mountpoint);
    println!();
    println!("Mounting... (Ctrl+C to unmount)");
    println!();

    let options = vec![
        MountOption::RW,
        MountOption::FSName("kriptofs".to_string()),
        MountOption::AutoUnmount,
        MountOption::AllowOther,
    ];

    let fs = PassthroughFS::new(source);
    
    fuser::mount2(fs, mountpoint, &options).unwrap();
}
