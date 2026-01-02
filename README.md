# BoringTun

[![crates.io](https://img.shields.io/crates/v/boringtun.svg)](https://crates.io/crates/boringtun)
[![crates.io](https://img.shields.io/crates/v/boringtun-cli.svg)](https://crates.io/crates/boringtun-cli)

**BoringTun** 是一个高性能、可移植的 [WireGuard<sup>®</sup>](https://www.wireguard.com/) 协议实现。

## 简介

BoringTun 已成功部署在数百万 iOS 和 Android 设备以及数千台 Cloudflare Linux 服务器上。

项目包含两个部分：

- **boringtun-cli**：Linux 和 macOS 的用户态 WireGuard 实现
- **boringtun**：可在各种平台（包括 iOS 和 Android）上实现快速高效的 WireGuard 客户端应用的库

## 安装

```bash
cargo install boringtun-cli
```

## 编译

```bash
# 仅编译库
cargo build --lib --no-default-features --release

# 编译可执行文件
cargo build --bin boringtun-cli --release
```

编译后的可执行文件位于 `./target/release` 目录。

## 使用

### 启动隧道

**方式一：使用 wg 工具配置**

```bash
# 1. 启动空隧道（wg0 是接口名称，不是配置文件）
sudo boringtun-cli -f wg0

# 2. 使用 wg 工具配置隧道
sudo wg setconf wg0 /path/to/wg0.conf
```

**方式二：使用 wg-quick（推荐）**

```bash
# wg0.conf 是标准 WireGuard 配置文件
sudo WG_QUICK_USERSPACE_IMPLEMENTATION=boringtun-cli wg-quick up wg0
```

### WireGuard 配置文件格式

`wg0.conf` 是标准 WireGuard 配置文件：

```ini
[Interface]
PrivateKey = <私钥>
Address = 10.0.0.2/24
DNS = 8.8.8.8

[Peer]
PublicKey = <对端公钥>
Endpoint = <对端地址>:端口
AllowedIPs = 0.0.0.0/0
PersistentKeepalive = 25
```

### 命令行参数

| 参数 | 说明 |
|------|------|
| `INTERFACE_NAME` | 接口名称（必需） |
| `-f, --foreground` | 前台运行 |
| `-t, --threads` | 线程数（默认 4） |
| `-v, --verbosity` | 日志级别：error/info/debug/trace |
| `--disable-connected-udp` | 禁用已连接的 UDP socket |
| `--proxy` | 上游代理地址，如 `127.0.0.1:1080` |
| `--proxy-type` | 代理类型：socks5/http（默认 socks5） |

### 代理支持

BoringTun 支持 SOCKS5 代理转发 WireGuard 流量：

```bash
# 使用 SOCKS5 代理
boringtun wg0 --proxy 127.0.0.1:1080 --proxy-type socks5
```

**注意**：
- 仅支持 SOCKS5 UDP ASSOCIATE 协议
- HTTP 代理不支持 UDP 流量，会自动回退到直接连接

## 平台支持

| 平台 | 可执行文件 | 库 |
|------|:---------:|:--:|
| Linux (x86_64, aarch64, armv7) | ✓ | ✓ |
| macOS (x86_64) | ✓ | ✓ |
| Windows (x86_64) | | ✓ |
| iOS (arm64, armv7) | | ✓ |
| Android (arm64, arm) | | ✓ |

### Linux

需要 `CAP_NET_ADMIN` 权限：

```bash
sudo setcap cap_net_admin+epi boringtun
```

如需使用 `fwmark`，请运行 `--disable-drop-privileges` 或设置 `WG_SUDO=1`。

### macOS

接口名称必须为 `utun[0-9]+` 或 `utun`（让内核自动选择）。

## 许可证

本项目基于 [3-Clause BSD License](https://opensource.org/licenses/BSD-3-Clause) 开源。

---

<sub><sub><sub><sub>WireGuard 是 Jason A. Donenfeld 的注册商标。BoringTun 未获得 Jason A. Donenfeld 的赞助或认可。</sub></sub></sub></sub>
