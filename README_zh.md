# narchive

[English](README.md)

一个使用 Rust 编写的高效、并发、具有弹性的网易云音乐命令行下载工具。

## 特性

- **并发下载**：支持自定义并发限制，同时下载多首歌曲。
- **动态 UI**：使用多进度条终端界面展示整体进度和单首歌曲下载进度。
- **元数据标注**：自动将歌词、封面图片以及元数据（包括网易云歌曲 ID）嵌入到下载的音轨文件中。
- **会话恢复**：通过本地会话目录与 `.narchive-dl` 配置文件，无缝恢复未完成的下载任务。
- **错误处理**：针对临时性网络错误实现 3 次带退避的自动重试，对受限或仅限 VIP 的歌曲则快速报错退出。

## 安装与配置

本项目需要运行中的网易云音乐 API 实例，推荐使用 [NeteaseCloudMusicApiEnhanced](https://github.com/neteasecloudmusicapienhanced/api-enhanced)。

1. 在项目根目录的 `.env` 文件中配置 API 端点 URL：
   ```env
   NETEASE_API=http://localhost:3000
   # 可选：配置 VIP/受限歌曲所需的 Cookie
   # COOKIE=your_cookie_here
   ```
2. 编译项目：
   ```bash
   cargo build --release
   ```

## 命令行参数

| 参数 | 环境变量 | 说明 |
| --- | --- | --- |
| `--api` | `NETEASE_API` | 网易云 API 服务端点 URL |
| `--cookie` | `USER_COOKIE` | 登录用户的 Cookie |
| `--download-path` | `DOWNLOAD_PATH` | 下载目标文件夹路径 |
| `--user-agent` | `USER_AGENT` | 自定义 User Agent |
| `--query-params` | `QUERY_PARAMS` | 自定义请求查询参数 (例如 `key=val&another=123`) |
| `--br` | `TARGET_BR` | 目标音轨比特率 (bps，例如 `320000`) |
| `--track` | — | 要下载的单曲 ID (可指定多个) |
| `--album` | — | 要下载的专辑 ID (可指定多个) |
| `--playlist` | — | 要下载的歌单 ID (可指定多个) |
| `--resume` | — | 要恢复下载的文件夹路径 |
| `--concurrent` | `CONCURRENT_DOWNLOADS` | 最大并发下载数 (默认：`3`) |
| `--no-metadata` | `NO_METADATA` | 不在下载的歌曲中嵌入文本元数据（标题、歌手、歌词等） |
| `--no-cover` | `NO_COVER` | 不在下载的歌曲中嵌入封面图片 |

## 使用方法

### 下载单曲、专辑或歌单
将对应的 ID 传给 `--track`、`--album` 或 `--playlist`。您可以指定自定义下载目录和并发限制：

```bash
./target/release/narchive --track 123456 --album 789012 --playlist 345678 --download-path ./downloads --concurrent 4
```

*注意：如果未指定下载路径，将在当前路径下自动创建一个名为 `narchive-<随机ID>` 的文件夹。*

### 恢复下载会话
通过指定包含 `.narchive-dl` 配置文件的目录来恢复未完成的下载任务：

```bash
./target/release/narchive --resume ./downloads
```
