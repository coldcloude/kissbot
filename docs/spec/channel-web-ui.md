# channel-web-ui 技术细节约定

## 预置后端配置（backends.json）

`kissbot-channel-web-ui` 使用 `public/backends.json` 文件存储预置后端列表，运行时通过 `fetch('/backends.json')` 加载，**不进入 JS bundle**。

### 文件格式

```json
{
  "backends": [
    { "name": "环境名称", "url": "https://api.example.com" }
  ]
}
```

- `name`（string）：显示在登录页的选项名称
- `url`（string）：后端 HTTP 地址，必须以 `http://` 或 `https://` 开头

### 部署替换

```bash
# 将生产环境的配置文件拷入构建产物覆盖
cp /path/to/production-backends.json dist/backends.json
```

- 替换后刷新页面即可生效，**无需重新构建**
- 至少保留一个条目，否则登录页降级为仅显示自定义输入
- 若不需要预置后端，可提供空数组 `{ "backends": [] }`
