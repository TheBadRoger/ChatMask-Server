# ChatMask 打洞协调服务器

只进行短期身份登记、来源地址观察和双端打洞信令。没有中继、聊天内容转发或离线代收。客户端与服务器通过 QUIC 建立经过 PeerId 认证的加密控制连接；服务器从该连接获取 UDP 来源地址，无需单独的探测套接字。

## 部署

服务器需有公网 IP，并放行 UDP 7001（云服务器同时开放安全组）。

```sh
docker compose up -d --build
docker compose logs -f
```

Compose 使用非 root 用户、只读根目录、持久化 server-data 卷、256 MiB 内存限制和日志轮转。服务器密钥位于 `/data/server.key`；重启保持 PeerId。备份密钥，不公开，不用于两个并行实例。不要用 `docker compose down -v` 删除身份卷。

直接构建运行（Rust 1.94 或更新版本；本机还需要 C 编译器和 CMake 构建 QUIC 加密依赖）：

```sh
cargo build --locked --release
./target/release/chatmask-server
```

Windows 使用 `target\release\chatmask-server.exe`。默认 `--listen /ip4/0.0.0.0/udp/7001/quic-v1`、`--key data/server.key`，可配置 `--max-clients`（256）和 `--ttl-seconds`（60）。IPv6 可用 `--listen /ip6/::/udp/7001/quic-v1`。Ctrl+C 或 Linux SIGTERM 正常退出；身份文件损坏时拒绝启动，不重新生成。

Linux 后台运行：

```sh
sudo useradd --system --no-create-home --shell /usr/sbin/nologin chatmask-server
sudo install -m 755 target/release/chatmask-server /usr/local/bin/chatmask-server
sudo install -m 644 deploy/chatmask-server.service /etc/systemd/system/chatmask-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now chatmask-server
sudo journalctl -u chatmask-server -f
```

日志输出 `Control address: /ip4/.../udp/7001/quic-v1/p2p/服务器PeerId`。分发给用户时将监听地址 `0.0.0.0` 或内网地址换成真实公网 IP，保留端口和服务器 PeerId。

## ChatMask 客户端接入

```sh
chatmask-client --bind 0.0.0.0 --server /ip4/服务器公网IP/udp/7001/quic-v1/p2p/服务器PeerId
```

双方需登记到至少一个共同服务器；`--server` 可重复指定最多四个带 PeerId 的 IP/QUIC 地址。使用 `/chat 对方PeerId`，无需先保存对方地址。客户端优先尝试已保存直连地址；无地址或拨号失败时请求协调。服务器给双方发送同一轮通知，双方复用原 QUIC UDP 监听端口同时拨号，直连成功后继续现有端到端消息加密、确认和待发补发。

服务器不会进入联系人或聊天会话。服务器不可用或打洞失败时，消息保持本地 Pending，不自动改走服务器转发。

## 协议和资源

协议为 `/chatmask/punch/3.0.0`，CBOR request-response，类型见 `src/lib.rs`：

1. 客户端建立经过认证的 QUIC 控制连接并发出 Register。
2. 服务器回复 Registered（观察到的 UDP 来源 endpoint、有效期）。身份来自连接认证，地址来自实际连接，不接受客户端声称的任意 endpoint。
3. A 请求 Connect（B的PeerId）。双方已登记时向双方发出 Punch（对端身份、endpoint、attempt、最长 12000ms），并向 A 回复 Started。Started 仅表示信令已安排，不表示打洞成功。
4. 客户端响应 Ack，并从原端口进行有限次数的同时拨号。实际 QUIC 连接必须验证预期 PeerId，聊天继续原有消息加密握手。
5. 客户端按服务器有效期刷新登记，默认每20秒；60秒过期或断线时移除登记。登记仅在内存，重启后需重新登记。

默认最多256条已建立连接/登记、每身份一条控制连接、32个待完成入站连接；控制包2048字节上限、每连接8个并发请求子流、5秒请求超时。每身份发起协调的间隔至少2秒，重复登记不会绕过该限制。

这不是标准 STUN/ICE、libp2p DCUtR 或 TURN 服务。使用 QUIC 的已认证来源地址和复用端口进行协调打洞。NAT可能针对不同目标产生不同映射，严格 NAT、CGNAT 或防火墙仍可能阻止直连；不保证公网成功率，不提供中继兜底。连接对象会获知彼此 IP，这是直连需要的信息；客户端会拒绝屏蔽目标的打洞通知。

## 测试与后续计划

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

服务器测试覆盖真实进程上的 QUIC 身份登记、来源地址与双端协调，以及容量、有效期、速率限制和密钥损坏拒绝。客户端项目另有需要本服务器二进制的真实双客户端加密消息测试。公网 NAT 成功率、Docker 和 Linux systemd 尚需实际部署验证。

未来节点探测、发现和 tracker peer 列表只列入 [PLAN.md](PLAN.md)，没有实现占位接口。

## 旧部署升级

现在只开放 UDP 7001；不再需要 TCP7000、`--udp-listen`、旧 JSON probe/签名确认或 `/chatmask/punch/1.0.0`。请同时升级客户端与服务器。旧密钥仍可复用，旧中继、旧服务和容器需先停止以避免端口冲突；旧数据卷不自动删除。

## 活动日志

服务端记录带 Unix 时间戳的连接、断开、节点登记、节点查询、失败原因、打洞通知发送与客户端确认、通知失败和登记过期。记录 PeerId、IP 和端口，不记录聊天正文或密钥。日志写入标准输出，由 Docker 保存和轮转。

查看最近日志：
```sh
docker compose logs --timestamps --tail=100 server
```
实时跟随输出（Ctrl+C 只停止查看日志）：
```sh
docker compose logs --timestamps --tail=100 -f server
```
前台启动显示实时输出：
```sh
docker compose up --build
```
更新源码后重建：`docker compose up -d --build`。

`punch_coordination_dispatched` 表示双方通知已提交，`punch_notice_response ... Ack` 表示客户端确认收到通知。服务器无法观察双方后续直连，不能据此宣布打洞或聊天成功，最终结果以客户端为准。

新版客户端使用 `/join 公网IP或域名` 一次添加服务器，之后每次启动自动连接和登记；`/peerid` 只显示本机 PeerId，双方用 `/chat 对方PeerId` 聊天。

## 锥形 NAT 与同网直连

协议升级为 `/chatmask/punch/3.0.0`，需要同时更新服务端与两个客户端，再执行 `docker compose up -d --build`。服务器密钥和已有配置可以保留，旧 2.0 客户端不兼容。

Register 携带最多 8 个客户端实际 QUIC 监听地址中的内网候选。服务器只保留私有单播地址及非零端口，公网端点仍从认证连接观察；公网来源不能注入回环地址，候选按身份登记、TTL 过期、断线清理。Connect 时仅将候选发送给本轮连接双方，不提供列表查询或中继。

新版客户端优先排列内网候选，同时探测公网端点；按双方 PeerId 确定唯一握手方向。一方用原 QUIC 监听套接字发送 UDP 探测并等待入站，另一方复用监听端口发起握手。单轮窗口 12 秒，QUIC 握手超时 10 秒。双方可通过内网可达路径绕过不支持 hairpin 的 NAT，但校园网设备隔离仍可能阻断。不同目标映射不同的对称 NAT、多层 NAT 不保证成功。

候选地址来自客户端声明，不能作为真实位置的证明；客户端仍验证目标 PeerId，限制候选数量、地址类型和尝试时间。双方会获知对方的私有地址及公网地址，日志也会记录候选。此实现不是完整 ICE/STUN、TURN 或 libp2p DCUtR。
