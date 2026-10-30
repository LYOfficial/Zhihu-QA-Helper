"""大模型调用层：统一走 OpenAI 兼容协议，Anthropic 单独适配。

刻意只依赖标准库（urllib/json），这样用户装完软件就能跑，
不需要先 pip install 一堆东西。
"""

from __future__ import annotations

import json
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


def chat(cfg: dict, messages: list[dict], temperature: float = 0.3,
         max_tokens: int = 4096, json_mode: bool = False) -> str:
    c = _norm(cfg)
    if not c["apiKey"] and c["provider"] != "custom":
        raise LLMError("还没有填写模型 API Key，请到「模型与账号」里配置")

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
        )
        blocks = res.get("content") or []
        return "".join(b.get("text", "") for b in blocks if b.get("type") == "text")

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
    res = _post(f"{c['baseUrl']}/chat/completions", payload, headers)
    try:
        return res["choices"][0]["message"]["content"] or ""
    except (KeyError, IndexError):
        raise LLMError(f"模型返回结构异常：{json.dumps(res, ensure_ascii=False)[:300]}") from None


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
    """从 models.dev 开源目录同步各主流厂商的模型 ID。"""
    raw = _get_json(MODEL_CATALOG_URL)
    out = {}
    for provider, source_names in CATALOG_SOURCES.items():
        ids = []
        for source in source_names:
            item = raw.get(source) if isinstance(raw, dict) else None
            models = item.get("models", {}) if isinstance(item, dict) else {}
            if isinstance(models, dict):
                for key, meta in models.items():
                    if isinstance(meta, dict) and meta.get("type") not in (None, "text"):
                        continue
                    if str(key).strip():
                        ids.append(str(key))
        unique = sorted(set(ids))
        if unique:
            out[provider] = unique[:80]
    if not out:
        raise LLMError("模型目录没有返回可用的主流模型")
    return {"providers": out, "source": MODEL_CATALOG_URL}


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
