# memory-store 记录写入

> 文件组织见 [memory-directory.md](memory-directory.md)，索引见 [memory-index.md](memory-index.md)
> channel 侧的消息推送见 [channel.md](channel.md)

## 统一写入框架

- 记忆存储只有 channel 一类记录：思考/工具内容以 `Content::Think` / `ToolCall` / `ToolResult` 直接内嵌在通道记录的 `content` 里，随该通道记录一起写入，不单独成记录、不单独落文件
- 通道记录由 `ChannelParser` 将请求解析为 (key, record)，同批请求按 key（agent + 记忆后缀 + 日期）分组后并行写入各文件
- channel 记录原样带入 `msg_id` 与可选 `meta`；归档与定位按通道分开调用，locate 不与已存记录去重（调用方在 locate 之后才写入新消息），见 [memory-directory.md](memory-directory.md)

## 文件状态与序号

- 每个 key 维护文件状态：已分配的最大序号（sn）和最后写入时间
- 进程重启后首次写入某文件时，倒序读取文件最后一行恢复状态
- sn 在文件内单调递增；同批记录按 (time, sn) 排序

## 乱序处理

- 判定：新批次的最小时间早于文件最后写入时间，即插入乱序
- 默认策略：拒绝写入，返回乱序错误
- force 策略：全量重写——读出文件全部记录，与新记录合并按 (time, sn) 重排，sn 重新编号后写回

## 索引联动

- 写入完成后通过 FileHook 通知索引模块（见 [memory-index.md](memory-index.md)）：
  - 追加写入 → 追加级别变更标记
  - 全量重写 → 重建级别变更标记
- 写入失败仅记录日志，不中断后续写入

## 新数据通知

- 通知服务在独立的 WS 监听地址上提供服务：`memory.store.notify_listen_addr` / `memory.store.notify_listen_port`（默认 `127.0.0.1:8182`），与 HTTP 服务分开监听（连接方式见 [communication.md](communication.md)）
- 握手认证与 HTTP 同一套（`X-Api-Key`）；服务端定期发送心跳，连接断开后移除客户端
- 写入完成后向所有已连接客户端广播新数据，不区分写入方式；通道记录按通道分组逐条通知（同一批可能混合多个通道）
- 通知携带位置信息：`payload_type` 为通道记录通知（`0x3001_0001`），payload 为时间、agent、记忆后缀（项目级 `RoleEvent { role_name, event_id }`）、发送者身份 `is_self` 与**通道三元组** `messenger_id` / `self_user_id` / `group_id`（平铺在 payload 顶层），订阅方据此只评估该通道
- 单个连接的握手/读写失败只记录日志，不影响服务继续接受连接
- 客户端按自身水位从共享文件读取；通知帧格式与消费方式见 [kissbot-memory-struct-abstract](memory-struct-abstract.md)
