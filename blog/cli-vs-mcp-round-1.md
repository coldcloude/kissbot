# CLI和MCP和SKILL

## 几个结论

1. 从图上看，MCP真的边缘化了，它只是Tool的浅封装，正如其名字，它主要作用就是注入上下文。

2. Tool也在边缘化，因为Tool不能由人直接使用，因此会被既能由人使用、又能由Agent使用的CLI替代。

3. Skill+CLI模式和Tool模式对比，最大的特征是解耦，Skill负责把CLI的描述注入到Agent的上下文，CLI负责干活。

4. 因此根据2和3，Skill+CLI模式有两大优势：（1）Skill可以优化上下文；（2）CLI可以被人调试
