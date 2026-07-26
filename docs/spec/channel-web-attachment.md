# channel-web 附件存储

> 附件的整体设计见 [channel-attachment.md](channel-attachment.md)

## key 与文件组织

- key 格式：`{group_id}/{uuid}`
- 文件结构：`{base_path}/{group_id}/` 下
  - `{uuid}`：附件本体
  - `{uuid}.metadata`：元数据（key、AttachmentInfo、有无缩略图），读取时走 LRU 缓存
  - `thumb_{uuid}`：缩略图

## 上传落盘

- 上传期间数据写入临时文件（`.{uuid}.uploading`），收满全部数据后转为正式文件；未完成的上传不产生正式文件

## 缩略图

- 图片附件上传完成后生成缩略图（200x200）
- 非图片请求缩略图返回错误

## 下载

- 按固定块大小（64KB）推送分块
