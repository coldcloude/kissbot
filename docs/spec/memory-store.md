# memory-store 记录写入

> 文件组织见 [memory-directory.md](memory-directory.md)，索引见 [memory-index.md](memory-index.md)
> channel 侧的消息推送见 [channel.md](channel.md)

## 统一写入框架

- 四类记录（channel / think / tool-call / tool-result）共用同一套写入框架，各自实例化批量追加器
- 每类记录由对应的 RequestParser 将请求解析为 (key, record)，同批请求按 key 分组后并行写入各文件

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
