# 协极串口录音分析 (Syner UART Recorder)

把 TL7218 固件 `WAV_RECORD_EN` 输出的 PCM 追踪流从串口录成 WAV,并支持回放。
Rust + [gpui](https://gpui.rs) 实现。

## 固件侧准备

在 <code>tl7218/app/_mesh_audio_/config.h</code> 中:

```c
#define WAV_RECORD_EN    1     // 默认 0
```

`USB_DEVICE_CLASS` 必须是 `CLASS_CDC`(当前配置已满足),否则
`bluetooth_config.h` 会直接 `#error`。重新编译烧录后,设备会枚举出一个
USB 虚拟串口。

默认运行时只有 `app/_mesh_audio_/intercom.c` 会写 WAV 包(`CODEC_MSG_TEST_AUDIO` 的音频定时器默认被注释掉,只有 2 ms 监控定时器工作):

| 位置 | 高 16 位 | 低 16 位 |
|---|---|---|
| `app/_mesh_audio_/intercom.c` | 降噪前(ASRC 输出) | 降噪后(NN NS 输出) |

> 只有在故意启用 `codec_tese_audio_timer` 时,`bluetooth/subsys/codec/codec_task.c` 才会额外写一路原始 mic 数据。普通使用不必关心。

## 构建与运行

```powershell
cargo run --release
```

首次构建会拉取 gpui 及其依赖,耗时较长。

> Windows 上如果反复出现 `failed to remove ...rcgu.o (os error 32)`,是杀软实时
> 扫描锁住了 rustc 刚写出的目标文件。把 `target/` 目录加入杀软排除项,或直接
> 重跑 `cargo build`(已编译的 crate 会缓存,重试能逐步收敛)。

## 界面

- **设置** — 选择串口(可刷新列表)、波特率、每包采样数、通道模式、采样率。
  参数点击即循环切换,自动持久化。录制中参数被锁定,避免写坏 WAV 头。
- **录音** — 开始/停止,并实时显示时长、包数、丢包、重同步字节、采样率、
  已接收字节、包头位宽,以及每通道峰值电平表。
- **记录** — 列出已录文件(时长/通道/采样率/大小),可播放、停止、删除。

## 参数说明

| 参数 | 含义 |
|---|---|
| 每包采样数 | 对应固件 `WAV_SAMPLES_PER_PACKET`,默认 160(10 ms @16 kHz)。设错会导致持续重同步、录不到数据。 |
| 通道模式 | 双通道(高+低)/ 仅高 16 位 / 仅低 16 位。做降噪 A/B 对比用双通道。 |
| 采样率 | 「自动」采用包头里的 `sample_rate` 字段;也可强制为 16000/8000。 |
| 波特率 | CDC 虚拟串口通常忽略,保留以便适配。 |

## 串口选择

TL7218 CDC 复合设备会枚举出两个虚拟 COM 口:

- **第一个 COM 口(接口 0)** — `usb_com_write`/`WAV_RECORD_EN` 走这里,工具要用这个口。
- **第二个 COM 口(接口 1)** — `cdc_console_flush` 输出,也就是 shell 命令行。

工具打开端口后会显式置位 **DTR + RTS**。固件 `usb_cdc_tx_request` 只有在 `cdc_line_state == 0x03` 时才会真正发送数据(`serialport-rs` 在 Windows 上默认不置这两根线,因此必须主动置位,否则会收到 0 字节)。

## 数据位置

- 设置:`%APPDATA%\syner-uart-recorder\settings.json`
- 录音:`%APPDATA%\syner-uart-recorder\recordings\<YYYYMMDD-HHMMSS>.wav`(UTC 时间戳)

## 诊断指标怎么读

- **丢包** > 0:固件端 6 包环形缓冲被覆盖(`sys_user_task` 主循环被阻塞),
  或 USB 侧丢数据。由包头 `idx`(模 4 计数)的跳变推算。
- **重同步字节** > 0:流中出现无法解析的字节。开始录制时通常有少量(接入
  时机处于包中间);持续增长说明「每包采样数」设置与固件不一致。

## 线上包格式

见 `src/proto.rs` 顶部注释与固件 `bluetooth/utilities/wav/wav.c`:

```text
offset  size  field
  0      2    preamble = 0xAAAA
  2      1    idx          包计数 & 0x03
  3      1    bits         16
  4      4    sample_rate  16000 (little endian)
  8    4*N    buffer[N]    每字 = (高16位 << 16) | 低16位
```

## 代码结构

| 文件 | 职责 |
|---|---|
| `src/proto.rs` | 包格式定义、增量解帧(支持任意分片与重同步)、丢包推算 |
| `src/recorder.rs` | 串口采集线程 → WAV 写入,状态计数发布 |
| `src/library.rs` | 扫描录音目录,读 WAV 头得到时长/通道/采样率 |
| `src/player.rs` | rodio 回放(音频设备懒初始化) |
| `src/settings.rs` | 参数与持久化路径 |
| `src/main.rs` | gpui 界面 |

`cargo test` 覆盖解帧、重同步、分片重组、丢包推算、参数循环与时间戳换算。
