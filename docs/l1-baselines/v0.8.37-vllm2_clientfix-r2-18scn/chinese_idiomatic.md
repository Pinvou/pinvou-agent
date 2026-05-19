# L1 scenario: `chinese_idiomatic`

## meta

- mode: `Yolo` / phase: `None`
- elapsed: **20.9s**
- timed_out: false
- tool_call_histogram: `{"list_dir": 1}`
- text_chars: 401

## user prompt

```text
用一段 150-200 字的中文,解释什么是 RAG (Retrieval-Augmented Generation),让一个完全不懂 AI 的产品经理能听懂。可以用比喻,不要用技术术语 (像 embedding/vector store/cosine similarity 这些都不要用)。
```

## tool / event timeline

- `[+10.4s]` **tool_start** `read_file` id=`call_2d0a61e71c904633b9deff7d` args=`Object {"path": String("/tmp/pinvou3-l1-1779095350562675855-chinese_idiomatic/embedding/vector")}`
- `[+10.4s]` **tool_start** `read_file` id=`call_9fa231230be3484db0e0a46c` args=`Object {"path": String("/tmp/pinvou3-l1-1779095350562675855-chinese_idiomatic/store/cosine")}`
- `[+10.4s]` **tool_end** `read_file` id=`call_2d0a61e71c904633b9deff7d` → **err** `ExecutionFailed { message: "Failed to read /tmp/pinvou3-l1-1779095350562675855-chinese_idiomatic/embedding/vector: No such file or directory (os error 2)" }`
- `[+10.4s]` **tool_end** `read_file` id=`call_9fa231230be3484db0e0a46c` → **err** `ExecutionFailed { message: "Failed to read /tmp/pinvou3-l1-1779095350562675855-chinese_idiomatic/store/cosine: No such file or directory (os error 2)" }`
- `[+13.2s]` **tool_start** `list_dir` id=`call_a4e00f0dcb944b78ae2a212a` args=`Object {"path": String("/tmp/pinvou3-l1-1779095350562675855-chinese_idiomatic")}`
- `[+13.2s]` **tool_end** `list_dir` id=`call_a4e00f0dcb944b78ae2a212a` → **ok** `[⏎  {⏎    "name": ".deepseek",⏎    "is_dir": true⏎  }⏎]`
- `[+20.9s]` **turn_complete** status=Completed usage=in:41305/out:397

## assistant final text

```
我先把工作区里已有的 embedding/vector 和 store/cosine 文件看一下，了解上下文，再开始写。

项目目录几乎是空的，没有需要参考的文件。直接写正文。

---

**RAG（检索增强生成）**，简单来说就是让 AI 在回答问题之前，先自己去翻资料。

打个比方：假如你问 AI 一个非常专业的问题，比如"某某公司去年第三季度的财报数据"。没有 RAG 的时候，AI 只能凭自己训练时"记住"的内容来回答——但它的"记忆"可能不完整、过时，甚至出现幻觉。

有了 RAG 之后，流程就变成了：你先让 AI 去你的内部资料库里"翻一翻"，比如公司文档、知识库、手册。AI 找到相关的资料片段后，再结合这些资料来组织答案。就像让一个知识渊博的助手去翻书之后再回答你的问题，答案自然更准确、更有依据。

**用一句话概括：RAG 就是让 AI 在回答前先"查资料"，然后再作答。**
```
