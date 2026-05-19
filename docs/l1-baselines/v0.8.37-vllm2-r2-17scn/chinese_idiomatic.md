# L1 scenario: `chinese_idiomatic`

## meta

- mode: `Yolo` / phase: `None`
- elapsed: **13.5s**
- timed_out: false
- tool_call_histogram: `{}`
- text_chars: 398

## user prompt

```text
用一段 150-200 字的中文,解释什么是 RAG (Retrieval-Augmented Generation),让一个完全不懂 AI 的产品经理能听懂。可以用比喻,不要用技术术语 (像 embedding/vector store/cosine similarity 这些都不要用)。
```

## tool / event timeline

- `[+13.5s]` **turn_complete** status=Completed usage=in:13555/out:232

## assistant final text

```
假设你是一个新来的产品经理，被要求写一份关于"公司过去五年产品策略"的报告。

**传统方式（纯生成）：**
你靠脑子硬想。脑子里记得多少就写多少，可能写得头头是道，但实际上有些细节是编的（比如某个产品的发布时间），有些重要信息你压根不知道。

**RAG 方式（检索 + 生成）：**
你拿到一个图书馆权限，里面放着公司所有的产品文档、会议纪要和市场报告。你先把这些资料翻一遍，挑出跟主题相关的部分，然后基于这些**真实资料**去写报告。

RAG 就是让 AI 也获得这种"图书馆权限"。它不再纯靠训练时死记硬背的信息来回答，而是：

1. 先**搜索**你的私有资料库，找出跟问题最相关的片段
2. 再**结合**这些片段来生成回答

这样的好处很明显：回答更准确、有据可查，而且随时能更新知识库，不用重新训练模型。就像一个博学的员工，只要给他最新的参考资料，他就能给出最新的、靠谱的答案。
```
