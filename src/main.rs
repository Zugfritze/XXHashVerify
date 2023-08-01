use crossbeam_channel::{bounded, Receiver};
use mimalloc::MiMalloc;
use std::collections::HashMap;
use std::env;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::Arc;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::task::JoinHandle;
use xxhash_verify::{compute_hash, export_all_hash, get_all_file_path, read_hash_file};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();
    let args = match Args::parse_args(&args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("解析参数时出现错误: {}", err);
            exit(1)
        }
    };

    // 创建任务信号量
    let task_semaphore = Arc::new(Semaphore::new(16));

    match args.model {
        Model::Check => {
            // 开始校验哈希
            let handles = model_check(args, task_semaphore);

            // 等待所有异步任务完成
            await_all_async_tasks(handles).await;
        }
        Model::Generate => {
            // 获取所有文件路径
            let file_paths = Arc::new(get_all_file_path(args.folder_path));

            // 开始计算哈希并发送到通道
            let (rx, handles) = model_generate(&file_paths, task_semaphore);

            // 创建哈希缓存
            let mut hash_cache = HashMap::new();

            // 从通道接收哈希并把哈希写入哈希缓存
            for (file_path, hash) in rx.iter().take(handles.len()) {
                hash_cache.insert(file_path, hash);
            }

            // 等待所有异步任务完成
            await_all_async_tasks(handles).await;

            // 把哈希缓存写入文件
            if let Err(err) = export_all_hash(
                args.hash_file_path,
                &hash_cache,
                &file_paths,
                args.folder_path,
            ) {
                eprintln!("写入哈希到文件时出现错误: {}", err);
                exit(1);
            };
        }
    }
}

enum Model {
    Generate,
    Check,
}

struct Args<'a> {
    model: Model,
    folder_path: &'a Path,
    hash_file_path: &'a Path,
}

impl Args<'_> {
    fn parse_args(args: &[String]) -> io::Result<Args> {
        let model = match args.get(1) {
            Some(model) => match model.as_str() {
                "-g" => Model::Generate,
                "-c" => Model::Check,
                _ => {
                    return Err(io::Error::new(
                        ErrorKind::Other,
                        format!("不支持的模式: {}", model),
                    ))
                }
            },
            None => return Err(io::Error::new(ErrorKind::Other, "缺少模式参数")),
        };
        let folder_path = match args.get(2) {
            Some(folder_path) => Path::new(folder_path),
            None => return Err(io::Error::new(ErrorKind::Other, "缺少文件夹路径参数")),
        };
        let hash_file_path = match args.get(3) {
            Some(hash_file_path) => Path::new(hash_file_path),
            None => return Err(io::Error::new(ErrorKind::Other, "缺少哈希文件路径参数")),
        };
        Ok(Args {
            model,
            folder_path,
            hash_file_path,
        })
    }
}

async fn request_task_permit(task_semaphore: &Semaphore) -> SemaphorePermit<'_> {
    match task_semaphore.acquire().await {
        Ok(permit) => permit,
        Err(err) => {
            eprintln!("获取任务信号量时出现错误: {}", err);
            exit(1);
        }
    }
}

async fn await_all_async_tasks(handles: Vec<JoinHandle<()>>) {
    for handle in handles {
        if let Err(err) = handle.await {
            eprintln!("等待异步任务完成时出现错误: {}", err);
            exit(1);
        }
    }
}

fn model_check(args: Args, task_semaphore: Arc<Semaphore>) -> Vec<JoinHandle<()>> {
    let hash_map = match read_hash_file(args.folder_path, args.hash_file_path) {
        Ok(hash_map) => hash_map,
        Err(err) => {
            eprintln!("读取哈希值时出现错误: {}", err);
            exit(1)
        }
    };

    let mut handles = Vec::new();

    for (file_path, hash) in hash_map {
        let task_semaphore = Arc::clone(&task_semaphore);

        let handle = tokio::spawn(async move {
            let permit = request_task_permit(&task_semaphore).await;

            match compute_hash(&file_path).await {
                Ok(hash_new) => {
                    if hash == hash_new {
                        println!("[{} | 成功]", file_path.display());
                    } else {
                        println!("[{} | 失败]", file_path.display());
                        exit(0);
                    }
                }
                Err(err) => {
                    if err.kind() == ErrorKind::NotFound {
                        println!("[{} | 缺失]", file_path.display());
                        exit(0);
                    } else {
                        println!("计算[{}]的哈希时出现错误: {}", file_path.display(), err);
                        exit(1);
                    }
                }
            }
            drop(permit);
        });
        handles.push(handle);
    }
    handles
}

fn model_generate(
    file_paths: &Arc<Vec<PathBuf>>,
    task_semaphore: Arc<Semaphore>,
) -> (Receiver<(PathBuf, u128)>, Vec<JoinHandle<()>>) {
    let (tx, rx) = bounded(64);
    let tx = Arc::new(tx);

    let mut handles = Vec::new();

    for file_path in file_paths.iter().cloned() {
        let tx = Arc::clone(&tx);
        let task_semaphore = Arc::clone(&task_semaphore);

        let handle = tokio::spawn(async move {
            let permit = request_task_permit(&task_semaphore).await;

            match compute_hash(&file_path).await {
                Ok(hash) => {
                    println!("[{} | {:x}]", file_path.display(), hash);
                    if let Err(err) = tx.send((file_path, hash)) {
                        eprintln!("发送哈希到通道时出现错误: {}", err);
                        exit(1)
                    }
                }
                Err(err) => {
                    eprintln!("计算[{}]的哈希时出现错误: {}", file_path.display(), err);
                    exit(1);
                }
            }
            drop(permit);
        });
        handles.push(handle);
    }
    (rx, handles)
}
