use fuser::{
    FileAttr, FileType, Filesystem, MountOption, ReplyAttr, ReplyDirectory,
    ReplyEntry, Request,
};
use libc::ENOENT;
use std::env;
use std::ffi::OsStr;
// //use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

struct PassthroughFS {
     #[allow(dead_code)]
    source: String,
}

impl PassthroughFS {
    fn new(source: String) -> Self {
        PassthroughFS { source }
    }
}

impl Filesystem for PassthroughFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        println!("lookup: parent={}, name={:?}", parent, name);
        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        println!("getattr: ino={}", ino);
        
        if ino == 1 {
            // Root directory
            let attr = FileAttr {
                ino: 1,
                size: 0,
                blocks: 0,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: FileType::Directory,
                perm: 0o755,
                nlink: 2,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                flags: 0,
                blksize: 512,
            };
            reply.attr(&TTL, &attr);
        } else {
            reply.error(ENOENT);
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
        
        if ino == 1 {
            if offset == 0 {
                let _ = reply.add(1, 0, FileType::Directory, ".");
                let _ = reply.add(1, 1, FileType::Directory, "..");
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() != 3 {
        eprintln!("Usage: {} <source> <mountpoint>", args[0]);
        eprintln!("Example: {} /mnt/kriptofs-storage /home/toni/Protected", args[0]);
        std::process::exit(1);
    }

    let source = &args[1];
    let mountpoint = &args[2];

    println!("KriptoFS POC v0.1");
    println!("Source: {}", source);
    println!("Mountpoint: {}", mountpoint);
    println!("Mounting... (Ctrl+C to unmount)");

    let options = vec![
        MountOption::RW,
        MountOption::FSName("kriptofs".to_string()),
        MountOption::AutoUnmount,
        MountOption::AllowOther,
    ];

    let fs = PassthroughFS::new(source.to_string());
    
    fuser::mount2(fs, mountpoint, &options).unwrap();
}
