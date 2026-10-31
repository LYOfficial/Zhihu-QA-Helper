"""大模型调用层：统一走 OpenAI 兼容协议，Anthropic 单独适配。

刻意只依赖标准库（urllib/json），这样用户装完软件就能跑，
不需要先 pip install 一堆东西。
"""

from __future__ import annotations

import json
import re
import time
import urllib.error
import urllib.request

PROVIDERS = {
    "openai": {
        "label": "OpenAI",
        "baseUrl": "https://api.openai.com/v1",
        "models": ["gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-astra", "gpt-6-luna", "gpt-5.6-sol"],
        "keyHint": "sk-...",
        "docs": "https://platform.openai.com/docs/models",
    },
    "anthropic": {
        "label": "Claude",
        "baseUrl": "https://api.anthropic.com/v1",
        "models": [
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-haiku-5-5",
            "claude-fable-5-1",
            "claude-mythos-5-1",
        ],
        "keyHint": "sk-ant-...",
        "docs": "https://platform.claude.com/docs/en/docs/about-claude/models",
    },
    "deepseek": {
        "label": "DeepSeek",
        "baseUrl": "https://api.deepseek.com/v1",
        "models": ["deepseek-flash", "deepseek-v4-pro"],
        "keyHint": "sk-...",
        "docs": "https://api-docs.deepseek.com/quick_start/pricing",
    },
    "glm": {
        "label": "智谱 GLM",
        "baseUrl": "https://open.bigmodel.cn/api/paas/v4",
        "models": ["glm-5", "glm-4.7", "glm-4.6", "glm-4-flash"],
        "keyHint": "xxxx.xxxx",
        "docs": "https://docs.bigmodel.cn",
    },
    "kimi": {
        "label": "Kimi",
        "baseUrl": "https://api.moonshot.cn/v1",
        "models": [
            "kimi-k3.1-pro",
            "kimi-k3.1-pro-highspeed",
            "kimi-k3",
            "kimi-k2.6",
        ],
        "keyHint": "sk-...",
        "docs": "https://platform.kimi.com/docs/get-api-key",
    },
    "qwen": {
        "label": "通义千问",
        "baseUrl": "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "models": ["qwen3.8-max", "qwen3.7-plus", "qwen3.8-flash", "qwen3.8-omni-flash"],
        "keyHint": "sk-...",
        "docs": "https://help.aliyun.com/zh/model-studio/models",
    },
    "minimax": {
        "label": "MiniMax",
        "baseUrl": "https://api.minimax.chat/v1",
        "models": ["MiniMax-M3.1", "MiniMax-M3.1-Flash", "MiniMax-M3"],
        "keyHint": "eyJ...",
        "docs": "https://platform.minimaxi.com",
    },
    "custom": {
        "label": "自定义 / 本地",
        "baseUrl": "http://127.0.0.1:11434/v1",
        "models": ["qwen2.5:14b"],
        "keyHint": "本地可留空",
        "docs": "",
    },
}

MODEL_CATALOG_URL = "https://models.dev/api.json"
CATALOG_SOURCES = {
    "openai": ["openai"],
    "anthropic": ["anthropic"],
    "deepseek": ["deepseek"],
    "glm": ["zhipuai"],
    "kimi": ["moonshotai"],
    "qwen": ["alibaba", "alibaba-cn"],
    "minimax": ["minimax"],
}


class LLMError(RuntimeError):
    pass


def _post(url: str, payload: dict, headers: dict, timeout: int = 180) -> dict:
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    for k, v in headers.items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")[:600]
        raise LLMError(f"HTTP {e.code}：{_friendly(e.code, body)}") from None
    except urllib.error.URLError as e:
        raise LLMError(f"网络不可达：{e.reason}") from None
    except TimeoutError:
        raise LLMError(f"模型 {timeout} 秒内没有返回，已放弃本次调用") from None
    except json.JSONDecodeError:
        raise LLMError("返回内容不是合法 JSON，可能是网关错误页") from None


def _get_json(url: str, timeout: int = 20):
    req = urllib.request.Request(url, method="GET", headers={"User-Agent": "Zhihu-QA-Helper/0.1"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        raise LLMError(f"模型目录同步失败：HTTP {e.code}") from None
    except urllib.error.URLError as e:
        raise LLMError(f"模型目录同步失败：网络不可达（{e.reason}）") from None
    except json.JSONDecodeError:
        raise LLMError("模型目录同步失败：返回内容不是合法 JSON") from None


def _friendly(code: int, body: str) -> str:
    if code == 401:
        return "API Key 无效或已过期"
    if code == 403:
        return "该 Key 没有访问此模型的权限"
    if code == 404:
        return f"接口地址或模型名不对。原始返回：{body}"
    if code == 429:
        return "触发限流或余额不足"
    return body


def _norm(cfg: dict) -> dict:
    provider = (cfg.get("provider") or "deepseek").strip()
    preset = PROVIDERS.get(provider, PROVIDERS["custom"])
    base = (cfg.get("baseUrl") or preset["baseUrl"]).rstrip("/")
    return {
        "provider": provider,
        "baseUrl": base,
        "model": (cfg.get("model") or preset["models"][0]).strip(),
        "apiKey": (cfg.get("apiKey") or "").strip(),
    }


def _approx_tokens(messages: list[dict]) -> int:
    """粗估输入 token：中文约 1 字 1 token，留 10% 余量。宁可高估，不要超窗。"""
    chars = sum(len(str(m.get("content") or "")) for m in messages)
    return int(chars * 1.1) + 16 * len(messages)


def _clamp_tokens(cfg: dict, messages: list[dict], want: int) -> int:
    """按模型真实容量收口 max_tokens。
    推理模型会先花 token 思考，给太小就会出现「正文为空/JSON 被截断」。
    models.dev 的 context / output 就是用来做这件事的。"""
    want = int(want or 4096)
    out_limit = int(cfg.get("outputLimit") or 0)
    if out_limit > 0:
        want = min(want, out_limit)
    ctx = int(cfg.get("contextLimit") or 0)
    if ctx > 0:
        avail = ctx - _approx_tokens(messages) - 512
        if avail > 0:
            want = min(want, avail)
    return max(256, want)


def chat(cfg: dict, messages: list[dict], temperature: float = 0.3,
         max_tokens: int = 4096, json_mode: bool = False, timeout: int = 180) -> str:
    c = _norm(cfg)
    if not c["apiKey"] and c["provider"] != "custom":
        raise LLMError("还没有填写模型 API Key，请到「模型与账号」里配置")
    max_tokens = _clamp_tokens(cfg, messages, max_tokens)

    if c["provider"] == "anthropic":
        system = "\n\n".join(m["content"] for m in messages if m["role"] == "system")
        conv = [m for m in messages if m["role"] != "system"]
        payload = {
            "model": c["model"],
            "max_tokens": max_tokens,
            "temperature": temperature,
            "messages": conv,
        }
        if system:
            payload["system"] = system
        res = _post(
            f"{c['baseUrl']}/messages",
            payload,
            {
                "x-api-key": c["apiKey"],
                "anthropic-version": "2023-06-01",
            },
            timeout=timeout,
        )
        blocks = res.get("content") or []
        text = "".join(b.get("text", "") for b in blocks if b.get("type") == "text")
        if not text.strip():
            raise LLMError(f"模型没有输出正文（stop_reason={res.get('stop_reason')}）")
        return text

    payload = {
        "model": c["model"],
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": False,
    }
    if json_mode:
        payload["response_format"] = {"type": "json_object"}
    headers = {"Authorization": f"Bearer {c['apiKey']}"} if c["apiKey"] else {}
    res = _post(f"{c['baseUrl']}/chat/completions", payload, headers, timeout=timeout)
    try:
        choice = res["choices"][0]
        text = (choice.get("message") or {}).get("content") or ""
        reason = choice.get("finish_reason")
    except (KeyError, IndexError):
        raise LLMError(f"模型返回结构异常：{json.dumps(res, ensure_ascii=False)[:300]}") from None
    if not text.strip():
        if reason == "length":
            raise LLMError(
                f"模型输出被截断（max_tokens={max_tokens} 全被推理占用）。请换非推理模型，或把上限调大"
            )
        raise LLMError(f"模型返回空正文（finish_reason={reason}）")
    return text


def _salvage(text: str) -> dict:
    """JSON 被截断时的兜底：至少把已经写出来的正文捞回来，不让整次拟稿白跑。"""
    out: dict = {}
    m = re.search(r'"markdown"\s*:\s*"', text)
    if m:
        start = m.end()
        buf = []
        i = start
        while i < len(text):
            ch = text[i]
            if ch == "\\" and i + 1 < len(text):
                esc = text[i + 1]
                buf.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\"}.get(esc, esc))
                i += 2
                continue
            if ch == '"':
                break
            buf.append(ch)
            i += 1
        body = "".join(buf).strip()
        if len(body) >= 80:
            out["markdown"] = body
    return out


def chat_json(cfg: dict, messages: list[dict], temperature: float = 0.3,
              max_tokens: int = 8000, retry_tokens: int = 16000,
              timeout: int = 180, min_chars: int = 0) -> dict:
    """要 JSON 的调用：一次不够就放大上限重试一次，最后做截断兜底。
    推理模型会把 max_tokens 花在 reasoning 上，所以这里默认给足余量。"""
    raw = chat(cfg, messages, temperature=temperature, max_tokens=max_tokens,
               json_mode=True, timeout=timeout)
    try:
        data = extract_json(raw)
        if not isinstance(data, dict):
            raise LLMError("模型返回的 JSON 不是对象")
        if min_chars and len(str(data.get("markdown") or "")) < min_chars:
            raise LLMError("正文过短")
        return data
    except LLMError as first:
        retry = list(messages)
        retry.append({"role": "assistant", "content": raw[:1200]})
        retry.append(
            {
                "role": "user",
                "content": "上面的输出被截断了。请重新输出一份完整且合法闭合的 JSON，"
                "不要省略字段，正文控制在要求字数下沿，确保最后一个大括号闭合。",
            }
        )
        raw2 = chat(cfg, retry, temperature=temperature, max_tokens=retry_tokens,
                    json_mode=True, timeout=timeout)
        try:
            data = extract_json(raw2)
            if isinstance(data, dict) and data:
                return data
        except LLMError:
            pass
        for candidate in (raw2, raw):
            saved = _salvage(candidate)
            if saved.get("markdown"):
                saved.setdefault("doubts", [])
                saved.setdefault("citations", [])
                saved["_salvaged"] = True
                return saved
        raise first


def list_models(cfg: dict) -> dict:
    """从平台自己的 /models 接口拉真实可用清单。
    硬编码的模型名迟早过期，这个接口才是唯一可靠的来源。"""
    c = _norm(cfg)
    if not c["apiKey"] and c["provider"] != "custom":
        raise LLMError("先填 API Key，才能拉取这个账号可用的模型清单")

    url = f"{c['baseUrl']}/models"
    req = urllib.request.Request(url, method="GET")
    if c["provider"] == "anthropic":
        req.add_header("x-api-key", c["apiKey"])
        req.add_header("anthropic-version", "2023-06-01")
    elif c["apiKey"]:
        req.add_header("Authorization", f"Bearer {c['apiKey']}")
    req.add_header("Accept", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=25) as resp:
            data = json.loads(resp.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        if e.code in (404, 405):
            raise LLMError(
                f"{c['provider']} 没有开放 /models 接口，请手动填写模型名"
            ) from None
        raise LLMError(f"HTTP {e.code}：{_friendly(e.code, e.read().decode('utf-8', 'replace')[:200])}") from None
    except urllib.error.URLError as e:
        raise LLMError(f"网络不可达：{e.reason}") from None

    items = data.get("data") if isinstance(data, dict) else data
    ids = []
    for it in items or []:
        mid = it.get("id") or it.get("name")
        if isinstance(mid, str) and mid:
            ids.append(mid)
    if not ids:
        raise LLMError("接口返回里没有模型列表，请手动填写模型名")
    ids.sort()
    return {"models": ids, "count": len(ids), "baseUrl": c["baseUrl"]}


def sync_catalog() -> dict:
    """从 models.dev 开源目录同步各主流厂商的模型 ID 与上下文容量。
    models.dev 的每个模型都带 limit.context / limit.output，
    这里一并取回：调用方据此决定一次性最多能塞多少 token。"""
    raw = _get_json(MODEL_CATALOG_URL)
    out = {}
    limits: dict[str, dict[str, dict]] = {}
    for provider, source_names in CATALOG_SOURCES.items():
        ids = []
        lim: dict[str, dict] = {}
        for source in source_names:
            item = raw.get(source) if isinstance(raw, dict) else None
            models = item.get("models", {}) if isinstance(item, dict) else {}
            if isinstance(models, dict):
                for key, meta in models.items():
                    if isinstance(meta, dict) and meta.get("type") not in (None, "text"):
                        continue
                    if not str(key).strip():
                        continue
                    ids.append(str(key))
                    if isinstance(meta, dict):
                        limit = meta.get("limit") or {}
                        ctx = limit.get("context") or limit.get("input")
                        outp = limit.get("output")
                        if isinstance(ctx, (int, float)) and ctx > 0:
                            lim[str(key)] = {
                                "context": int(ctx),
                                "output": int(outp) if isinstance(outp, (int, float)) and outp > 0 else 0,
                            }
        unique = sorted(set(ids))
        if unique:
            out[provider] = unique[:80]
            limits[provider] = {m: lim[m] for m in unique[:80] if m in lim}
    if not out:
        raise LLMError("模型目录没有返回可用的主流模型")
    return {"providers": out, "limits": limits, "source": MODEL_CATALOG_URL}


def test_model(cfg: dict) -> dict:
    """真的发一次最小请求，而不是只检查 URL 拼写。"""
    started = time.time()
    try:
        out = chat(
            cfg,
            [
                {"role": "system", "content": "你是连通性测试。只回复两个字：可用"},
                {"role": "user", "content": "ping"},
            ],
            temperature=0,
            max_tokens=16,
        )
    except LLMError as e:
        return {
            "ok": False,
            "latencyMs": int((time.time() - started) * 1000),
            "message": str(e),
            "reply": "",
        }
    ms = int((time.time() - started) * 1000)
    return {
        "ok": True,
        "latencyMs": ms,
        "message": f"连接正常，首字延迟 {ms} ms",
        "reply": (out or "").strip()[:60],
    }


def extract_json(text: str) -> dict:
    """模型经常在 JSON 外面包一层 ```json，这里做鲁棒提取。"""
    text = (text or "").strip()
    if not text:
        raise LLMError("模型返回了空内容")
    if text.startswith("```"):
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip().startswith("```"):
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass
    start = text.find("{")
    depth = 0
    for i in range(start if start >= 0 else 0, len(text)):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                try:
                    return json.loads(text[start:i + 1])
                except json.JSONDecodeError:
                    break
    raise LLMError(f"无法解析模型输出的 JSON：{text[:300]}")
