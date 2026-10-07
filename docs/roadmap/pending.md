# 遗留事项

## 流程变更

1. [x] 扩展trigger类型，增加无须输入和触发机制的Trigger。所有类型的trigger均可以加载复数个。去掉AgentTrigger结构体，建立一个超Pipeline结构体，保存AgentPipeline和各类型Trigger的集合

2. [ ] 增加auto_bind

## 处理优化

1. [ ] 排查DashMap使用合理性
2. [ ] RoleEvent去掉构造函数
3. [ ] Request结构体改用非Arc字段，使用所有权转移代替复制
4. [ ] 检查ArcStr::from排查可去除复制
5. [ ] memory record乱序时，不重写整个文件，而是回溯到非乱序的记录，仅重写乱序+新记录
