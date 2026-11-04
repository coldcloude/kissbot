# 代码持续优化

## 长期通用
- 排查DashMap使用合理性
- Request结构体改用非Arc字段，使用所有权转移代替复制
- 检查ArcStr::from排查可去除复制

## 进行中
- RoleEvent去掉构造函数

## 计划中
- memory record乱序时，不重写整个文件，而是回溯到非乱序的记录，仅重写乱序+新记录
