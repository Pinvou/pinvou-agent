# L1 scenario: `chinese_idiomatic`

## meta

- mode: `Yolo` / phase: `None`
- elapsed: **12.5s**
- timed_out: false
- tool_call_histogram: `{}`
- text_chars: 331

## user prompt

```text
用一段 150-200 字的中文,解释什么是 RAG (Retrieval-Augmented Generation),让一个完全不懂 AI 的产品经理能听懂。可以用比喻,不要用技术术语 (像 embedding/vector store/cosine similarity 这些都不要用)。
```

## tool / event timeline

- `[+12.5s]` **turn_complete** status=Completed usage=in:13555/out:180

## assistant final text

```
好的，我来写一段通俗解释 RAG 的文字。

---

**什么是 RAG？**

想象一下，你公司的客服机器人就像一个刚入职的新人。他背下了大量通用知识，但对你们公司的具体业务并不了解。当他回答问题时，可能会说一些听起来很有道理但实际上并不准确的话。

RAG 就像给这位新人配了一本"参考手册"。每当有人提问时，机器人不会只凭自己的记忆作答，而是先去参考手册里查找最相关的资料，仔细阅读后再组织答案。这样一来，它回答的问题不仅流畅自然，还能精准引用公司内部的政策、数据等信息，大大降低了"胡说八道"的风险。

简单来说，RAG 就是让 AI 在回答问题前先"翻翻书"，而不是"凭感觉瞎编"。这样既保留了 AI 强大的表达能力，又让它能给出有据可依的准确答案。
```
