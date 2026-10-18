#[tokio::main]
async fn main() {
    // 支持 `--config <path>` / `--config=<path>`，其余参数忽略
    let args: Vec<String> = std::env::args().collect();
    let mut index = 1;
    while index < args.len() {
        if args[index] == "--config" && index + 1 < args.len() {
            std::env::set_var("ACC_CONFIG", &args[index + 1]);
            index += 2;
            continue;
        }
        if let Some(path) = args[index].strip_prefix("--config=") {
            std::env::set_var("ACC_CONFIG", path);
        }
        index += 1;
    }

    if let Err(err) = acc_server::run().await {
        eprintln!("启动失败: {err:#}");
        std::process::exit(1);
    }
}
