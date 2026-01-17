use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory,
    ReplyEntry, ReplyOpen, ReplyData, Request,
};
use libc::ENOENT;
use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

struct PassthroughFS {
    source: PathBuf,
    inode_map: Mutex<HashMap<u64, PathBuf>>,
    next_inode: Mutex<u64>,
}

impl PassthroughFS {
    fn new(source: PathBuf) -> Self {
        let mut inode_map = HashMap::new();
        inode_map.insert(1, source.clone());
        
        PassthroughFS {
            source,
            inode_map: Mutex::new(inode_map),
            next_inode: Mutex::new(2),
        }
    }

    fn get_inode(&self, path: &Path) -> u64 {
        let mut map = self.inode_map.lock().unwrap();
        
        for (ino, p) in map.iter() {
            if p == path {
                return *ino;
            }
        }
        
        let mut next = self.next_inode.lock().unwrap();
        let ino = *next;
        *next += 1;
        map.insert(ino, path.to_path_buf());
        ino
    }

    fn get_path(&self, ino: u64) -> Option<PathBuf> {
        let map = self.inode_map.lock().unwrap();
        map.get(&ino).cloned()
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

                let ino = self.get_inode(path);

                Ok(FileAttr {
                    ino,
                    size: metadata.len(),
                    blocks: metadata.blocks(),
                    atime: metadata.accessed().unwrap_or(UNIX_EPOCH),
                    mtime: metadata.modified().unwrap_or(UNIX_EPOCH),
                    ctime: SystemTime::now(),
                    crtime: UNIX_EPOCH,
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
        
        let parent_path = match self.get_path(parent) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
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
        
        let path = match self.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
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
        
        let path = match self.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        match fs::File::open(&path) {
            Ok(mut file) => {
                use std::io::Seek;
                
                if file.seek(std::io::SeekFrom::Start(offset as u64)).is_err() {
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
        
        let path = match self.get_path(ino) {
            Some(p) => p,
            None => {
                reply.error(ENOENT);
                return;
            }
        };
        
        let entries: Vec<_> = match fs::read_dir(&path) {
            Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
            Err(_) => {
                reply.error(ENOENT);
                return;
            }
        };

        let mut current_offset = 0i64;

        if offset <= current_offset {
            let _ = reply.add(ino, current_offset + 1, FileType::Directory, ".");
        }
        current_offset += 1;

        if offset <= current_offset {
            let _ = reply.add(ino, current_offset + 1, FileType::Directory, "..");
        }
        current_offset += 1;

        for entry in entries.iter() {
            if offset <= current_offset {
                let entry_path = entry.path();
                
                if let Ok(metadata) = entry.metadata() {
                    let kind = if metadata.is_dir() {
                        FileType::Directory
                    } else {
                        FileType::RegularFile
                    };
                    
                    let entry_ino = self.get_inode(&entry_path);
                    
                    let full = reply.add(
                        entry_ino,
                        current_offset + 1,
                        kind,
                        entry.file_name(),
                    );
                    
                    if full {
                        break;
                    }
                }
            }
            current_offset += 1;
        }
        
        reply.ok();
    }

    fn open(&mut self, _req: &Request, _ino: u64, _flags: i32, reply: ReplyOpen) {
        reply.opened(0, 0);
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
    println!("KriptoFS POC v0.3 - Fixed Inode");
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
    ];

    let fs = PassthroughFS::new(source);
    
    fuser::mount2(fs, mountpoint, &options).unwrap();
}
