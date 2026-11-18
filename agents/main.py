"""知乎答主助手 · 本地智能体服务

职责：一切需要大模型参与的活儿——
  1. /v1/profile   读历史内容，归纳领域画像
  2. /v1/sources   按领域检索权威平台
  3. /v1/research  检索资料 → 取图 → 起草 → 逐条核疑
  4. /v1/test      验证模型 API 连通性

只用标准库（http.server + urllib），随软件启动，用户无需 pip install。
默认监听 127.0.0.1:8788，只接受本机请求。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import llm  # noqa: E402
import prompts  # noqa: E402
import research  # noqa: E402
import illustrations
import investigation
import zhihu_open_search

VERSION = "0.1.0"
DATA_DIR = os.path.join(os.getcwd(), "data")


# ---------------------------------------------------------------- profile

def _str_list(value, limit: int) -> list[str]:
    if not isinstance(value, list):
        return []
    out = []
    for item in value:
        text = str(item).strip()
        if text and text not in out:
            out.append(text)
        if len(out) >= limit:
            break
    return out


STYLE_FIELDS = (
    "voice", "structure", "sentence", "pacing", "formatting", "examples", "length",
)


def _norm_style(raw) -> dict:
    """把模型给的文风档案收成固定结构，缺字段填空串，前端与提示词都按同一套读。"""
    raw = raw if isinstance(raw, dict) else {}
    style = {k: str(raw.get(k) or "").strip() for k in STYLE_FIELDS}
    style["openers"] = _str_list(raw.get("openers"), 6)
    style["signature"] = _str_list(raw.get("signature"), 6)
    style["avoid"] = _str_list(raw.get("avoid"), 4)
    style["variants"] = investigation.strings(raw.get("variants"), 6, 480)
    return style


def _writing_examples(samples: list[dict], question: dict) -> str:
    target = research.keywords(str(question.get("title") or "") + " " + " ".join(question.get("tags") or []))
    ranked = sorted(samples, key=lambda sample: (
        sample.get("kind") != "answer",
        -len(target & research.keywords(str(sample.get("title") or "") + " " + str(sample.get("text") or "")[:350])),
    ))
    closest = ranked[:2]
    others = [sample for sample in ranked[2:] if sample.get("kind") != (closest[0].get("kind") if closest else None)]
    selected = (closest + others[:1] + [sample for sample in ranked[2:] if sample not in others[:1]])[:4]
    return "\n\n".join(
        f"样本 {index}｜{'回答' if sample.get('kind') == 'answer' else '文章'}｜{sample.get('title', '')}\n{sample.get('text', '')[:1800]}"
        for index, sample in enumerate(selected, start=1)
    ) or "（没有答主真人样本；不虚构个人经历）"


def _retain_examples(samples: list[dict]) -> list[dict]:
    examples, seen = [], set()
    answers = [sample for sample in samples if sample.get("kind") == "answer"]
    articles = [sample for sample in samples if sample.get("kind") != "answer"]
    ordered = answers[:10] + articles[:6] + answers[10:] + articles[6:]
    for sample in ordered:
        text = research.clean_sample(sample.get("text") or "")
        if len(text) < 100 or text[:160] in seen:
            continue
        seen.add(text[:160])
        if len(text) > 1800:
            text = text[:1150] + "\n[…]\n" + text[-550:]
        examples.append({"kind": sample.get("kind"), "title": str(sample.get("title") or "")[:140], "text": text})
        if len(examples) == 16:
            break
    return examples


def do_profile(payload: dict) -> dict:
    samples = payload.get("samples") or []
    cfg = payload.get("model") or {}
    if not samples:
        raise llm.LLMError("没有可分析的历史内容")

    lines = []
    samples_for_style = _retain_examples(samples)
    analyzed = sorted(samples, key=lambda sample: sample.get("kind") != "answer")[:24]
    popular = [{"kind": sample.get("kind"), "title": sample.get("title", ""),
                "voteups": sample.get("voteups", 0),
                "text": research.clean_sample(sample.get("text") or "")[:240]} for sample in analyzed]
    longer = [sample for sample in samples_for_style if sample.get("kind") != "answer"][:4]
    longer.extend(sample for sample in samples_for_style if sample not in longer)
    for sample, size in [(item, 240) for item in popular] + [(item, 740) for item in longer[:12]]:
        head = "回答" if sample.get("kind") == "answer" else "文章"
        body = sample["text"]
        if len(body) > size:
            body = body[:size * 2 // 3] + " […] " + body[-size // 3:]
        votes = f"赞同 {sample['voteups']}｜" if "voteups" in sample else "文风节选｜"
        lines.append(f"- [{head}] {sample.get('title', '')}｜{votes}{body}")
    block = "\n".join(lines)

    try:
        data = llm.chat_json(
            cfg,
            [
                {"role": "system", "content": prompts.PROFILE_SYSTEM},
                {"role": "user", "content": f"以下是该答主的 {len(samples)} 条历史内容：\n\n{block}"},
            ],
            temperature=0.2,
            max_tokens=10000,
            retry_tokens=16000,
            timeout=150,
        )
    except llm.LLMError:
        data = llm.chat_json(
            cfg,
            [
                {"role": "system", "content": prompts.PROFILE_COMPACT_SYSTEM},
                {"role": "user", "content": f"历史内容：\n{block}"},
            ],
            temperature=0.1,
            max_tokens=5000,
            retry_tokens=8000,
            timeout=90,
        )

    terms = []
    for t in data.get("terms") or []:
        label = (t.get("label") or "").strip()
        if not label:
            continue
        kind = t.get("kind") if t.get("kind") in ("primary", "sub", "adjacent") else "sub"
        try:
            weight = float(t.get("weight") or 0.6)
        except (TypeError, ValueError):
            weight = 0.6
        terms.append(
            {
                "kind": kind,
                "label": label,
                "weight": max(0.05, min(1.0, weight)),
                "keywords": [str(k).strip() for k in (t.get("keywords") or []) if str(k).strip()][:10],
                "note": (t.get("note") or "").strip(),
            }
        )
    if not terms:
        raise llm.LLMError("模型没有给出可用的领域词条")

    return {
        "summary": (data.get("summary") or "").strip(),
        "evidence": [str(e) for e in (data.get("evidence") or [])][:6],
        "terms": terms,
        "style": _norm_style(data.get("style")),
        "samples": samples_for_style,
        "sampleSize": len(samples),
    }


# ---------------------------------------------------------------- sources

def do_sources(payload: dict) -> dict:
    domains = payload.get("domains") or []
    cfg = payload.get("model") or {}
    if not domains:
        raise llm.LLMError("领域列表为空")

    try:
        data = llm.chat_json(
            cfg,
            [
                {"role": "system", "content": prompts.SOURCES_SYSTEM},
                {
                    "role": "user",
                    "content": "领域：" + "、".join(domains) + "\n请给出这些领域的权威平台清单。",
                },
            ],
            temperature=0.2,
            max_tokens=3000,
            retry_tokens=5000,
            timeout=90,
        )
    except llm.LLMError:
        data = {}
    out = []
    for s in (data.get("sources") or [])[:8]:
        name = (s.get("name") or "").strip()
        base = (s.get("baseUrl") or "").strip()
        if not name or not base.startswith("http"):
            continue
        use_for = [u for u in (s.get("useFor") or ["fact"]) if u in ("fact", "image", "data")]
        out.append(
            {
                "domain": (s.get("domain") or domains[0]).strip(),
                "name": name,
                "baseUrl": base,
                "kind": s.get("kind") or "wiki",
                "trust": int(s.get("trust") or 3),
                "useFor": use_for or ["fact"],
                "notes": (s.get("notes") or "").strip(),
            }
        )
    if not out:
        minecraft = any(
            key in " ".join(domains).lower()
            for key in ("minecraft", "我的世界", "红石", "mc")
        )
        if minecraft:
            out = [
                {
                    "domain": domains[0],
                    "name": "Minecraft Wiki",
                    "baseUrl": "https://minecraft.wiki/",
                    "kind": "wiki",
                    "trust": 5,
                    "useFor": ["fact", "image"],
                    "notes": "查阅方块、实体、机制和版本变更。",
                },
                {
                    "domain": domains[0],
                    "name": "Minecraft 官方网站",
                    "baseUrl": "https://www.minecraft.net/",
                    "kind": "official",
                    "trust": 5,
                    "useFor": ["fact"],
                    "notes": "查阅官方版本公告和产品信息。",
                },
            ]
        else:
            out = [
                {
                    "domain": domains[0],
                    "name": "维基百科",
                    "baseUrl": "https://zh.wikipedia.org/",
                    "kind": "wiki",
                    "trust": 4,
                    "useFor": ["fact"],
                    "notes": "作为领域基础事实的起点，重要结论需交叉验证。",
                }
            ]
    return {"sources": out}


# ---------------------------------------------------------------- research

def _fmt_style(style) -> str:
    """把画像里的文风档案摊成提示词块。字段缺了就跳过，不要留空标题。"""
    if not isinstance(style, dict):
        return "（暂无文风档案，按老练答主的常规写法作答）"
    labels = (
        ("voice", "语气与人称"),
        ("structure", "文章结构的差异"),
        ("sentence", "句子节奏"),
        ("pacing", "详略取舍"),
        ("formatting", "排版习惯"),
        ("examples", "论据偏好"),
        ("length", "常见篇幅"),
    )
    parts = [
        f"- {label}：{str(style.get(key) or '').strip()}"
        for key, label in labels
        if str(style.get(key) or "").strip()
    ]
    for key, label in (("variants", "不同场景的文风观察"), ("openers", "常用开头"), ("signature", "口头禅"), ("avoid", "较少出现的写法")):
        items = investigation.strings(style.get(key), 8, 600)
        if items:
            parts.append(f"- {label}：" + "；".join(items))
    return "\n".join(parts) if parts else "（暂无文风档案，按老练答主的常规写法作答）"


def _strip_images(markdown: str) -> str:
    markdown = re.sub(r"^[ \t]*\[IMG:[^\]]*\][ \t]*\r?\n?", "", markdown, flags=re.M)
    markdown = re.sub(r"\[IMG:[^\]]*\]|<img\b[^>]*>", "", markdown, flags=re.I)
    markdown = re.sub(r"!\[[^\]]*\]\([^)]*\)", "", markdown)
    markdown = re.sub(r"[ \t]{2,}", " ", markdown)
    markdown = re.sub(r"\n{3,}", "\n\n", markdown)
    return markdown.strip()


def collect_material(payload: dict) -> dict:
    question = payload.get("question") or {}
    sources = investigation.source_catalog(question, payload.get("sources"))
    plan = investigation.plan_question(question, payload.get("model") or {}, sources)
    docs = investigation.gather_queries(plan["queries"], sources, time.time() + 23, hotspot=plan["hotspot"])
    warnings = [] if docs else ["已联网检索，但没有取得可引用的正文。初稿不会把题目前提当事实。"]
    return {"docs": docs, "warnings": warnings, "plan": plan}


def qualify_material(payload: dict) -> dict:
    material = payload.get("material") or {}
    question = payload.get("question") or {}
    docs = material.get("docs") or []
    plan = material.get("plan") or investigation.fallback_plan(question)
    claims = plan.get("claims") or []
    sources = investigation.source_catalog(question, payload.get("sources"))
    warnings = list(material.get("warnings") or [])
    try:
        findings, docs = investigation.investigate_claims(claims, docs, sources, payload.get("model") or {},
                                                          budget=investigation.verification_budget(payload))
    except llm.LLMError:
        findings = investigation.verify(claims, docs, {})
        warnings.append("模型未完成前置核查；请查看原文并检查初稿中的条件分析。")
    return {**material, "docs": docs, "plan": plan, "findings": findings, "warnings": warnings}


def _draft_references(markdown, entries, exposed):
    by_url = {doc["sourceUrl"]: key for key, (doc, _) in exposed.items()}
    notes, ordered = {}, []
    for entry in entries[:12]:
        if not isinstance(entry, dict):
            continue
        key = entry.get("id") or by_url.get(str(entry.get("url") or ""))
        if isinstance(key, str) and key in exposed:
            notes.setdefault(key, str(entry.get("snippet") or "").strip()[:320])

    def number(match):
        key = match[1]
        if key not in exposed:
            return ""
        if key not in ordered:
            ordered.append(key)
        return f"[{ordered.index(key) + 1}]"

    markdown = re.sub(r"\[\[(S\d+)\]\]", number, markdown)
    inline_count = len(ordered)
    ordered.extend(key for key in notes if key not in ordered)
    citations = []
    for key in ordered:
        doc, _ = exposed[key]
        url = doc["sourceUrl"]
        citations.append({"label": doc["title"] or url, "url": url,
                          "sourceName": doc["sourceName"], "snippet": notes.get(key, "")})
    if inline_count:
        refs = "\n".join(f"[{index}] {citation['label']} — {citation['url']}"
                         for index, citation in enumerate(citations[:inline_count], 1))
        markdown += "\n\n### 参考资料\n\n" + refs
    return markdown, citations


def _edit_draft(markdown, cfg, user, timeout):
    def protected(text):
        return ([line for line in text.splitlines() if re.search(r"\[\[?S\d+\]\]?", line, re.I)],
                sorted(re.findall(r"[0-9]+(?:[.\-/][0-9]+)*%?", text)),
                re.findall(r"```[\s\S]*?```|https?://[^\s)]+", text))

    try:
        result = llm.chat_json(cfg, [{"role": "system", "content": prompts.STYLE_EDIT_SYSTEM},
                                    {"role": "user", "content": user + "\n\n【待编辑初稿】\n" + markdown}],
                               temperature=0, max_tokens=min(9000, max(2000, len(markdown) * 3)),
                               retry_tokens=0, timeout=timeout)
    except llm.LLMError:
        return markdown
    edited = result.get("markdown")
    if (isinstance(edited, str) and not result.get("_salvaged") and
            len(markdown) * 0.7 <= len(edited) <= len(markdown) * 1.12 and protected(edited) == protected(markdown)):
        return edited.strip()
    return markdown


def write_draft(payload: dict) -> dict:
    started = time.monotonic()
    question, profile, cfg = (payload.get(key) or {} for key in ("question", "profile", "model"))
    material = payload.get("material") or {}
    settings = payload.get("settings") or {}
    draft_min = int(settings.get("draftMin", 500))
    draft_max = int(settings.get("draftMax", 1000))
    examples = profile.get("samples") or []
    warnings = list(material.get("warnings") or [])
    if not examples:
        warnings.append("没有答主原文样本；更新领域画像后，新初稿会参考真实回答。")
    findings = list(material.get("findings") or [])
    supported = {entry.get("url") for item in findings for entry in item.get("evidence", [])}
    ranked_docs = sorted(material.get("docs") or [], key=lambda doc: doc["sourceUrl"] not in supported)
    docs, exposed = investigation.fmt_docs(ranked_docs, investigation.question_text(question))
    by_url = {doc["sourceUrl"]: key for key, (doc, _) in exposed.items()}
    summary = "\n".join(
        f"- {item['text']} → {item['status']}：{item['verdict']} " +
        " ".join(f"[{by_url[entry['url']]}]" for entry in item.get("evidence", []) if entry.get("url") in by_url)
        for item in findings
    ) or "（没有前置结论；按资料限定适用范围。）"
    user = prompts.DRAFT_USER.format(
        title=question.get("title", ""),
        excerpt=(question.get("excerpt") or "（无补充说明）")[:2400],
        tags="、".join(question.get("tags") or []) or "（无）",
        profile=(profile.get("summary") or "（暂无画像）")[:1000],
        style=_fmt_style(profile.get("style")),
        examples=_writing_examples(examples, question),
        docs=docs,
        findings=summary,
    )
    data = llm.chat_json(cfg, [
        {"role": "system", "content": prompts.DRAFT_SYSTEM.format(draft_min=draft_min, draft_max=draft_max)},
        {"role": "user", "content": user},
    ], temperature=float(cfg.get("temperature", 0.3)),
        max_tokens=max(3000, min(12000, draft_max * 3 + 1400)),
        retry_tokens=0, timeout=125, min_chars=max(60, draft_min // 4))
    markdown = data.get("markdown")
    markdown = _strip_images(markdown.strip()) if isinstance(markdown, str) else ""
    if not markdown:
        raise llm.LLMError("模型没有返回有效正文。资料已保存，可以直接重试拟稿。")
    if data.get("_salvaged"):
        warnings.append("模型返回不完整，已保留可读正文。请检查结尾是否完整。")
    else:
        remaining = min(35, int(137 - (time.monotonic() - started)))
        if remaining >= 12:
            markdown = _strip_images(_edit_draft(markdown, cfg, user, remaining))
    doubts = list(findings)
    covered = {item["text"] for item in doubts}
    for entry in (data.get("doubts") or [])[:4]:
        text = entry.get("text") if isinstance(entry, dict) else entry
        if isinstance(text, str) and (text := text.strip()[:360]) and text not in covered:
            doubts.append({"text": text, "status": "unverified", "verdict": "等待联网复核", "source": ""})
            covered.add(text)
    entries = data.get("citations")
    markdown, citations = _draft_references(markdown, entries if isinstance(entries, list) else [], exposed)
    return {"draft": {"markdown": markdown, "doubts": doubts, "citations": citations}, "warnings": warnings}


def review_material(payload: dict) -> dict:
    material = payload.get("material") or {}
    docs, doubts = material.get("docs") or [], payload.get("doubts") or []
    sources = investigation.source_catalog(payload.get("question") or {}, payload.get("sources"))
    model = payload.get("model") or {}
    pending = [index for index, doubt in enumerate(doubts) if doubt.get("status") in ("unverified", "open")]
    claims = [doubts[index]["text"] for index in pending]
    if not model.get("apiKey") or not model.get("model"):
        return {"doubts": doubts}
    suggestions = investigation.focused_queries([query for index in pending for query in doubts[index].get("queries", [])], "")
    results, _ = investigation.investigate_claims(claims, docs, sources, model, budget=investigation.verification_budget(payload),
                                                  after_draft=True, suggested=suggestions)
    updated = list(doubts)
    for index, result in zip(pending, results):
        prior = doubts[index]
        if result["confidence"] >= (prior.get("confidence") or 0):
            updated[index] = {**prior, **result}
    return {"doubts": updated}



def collect_images(payload: dict) -> dict:
    material = payload.get("material") or {}
    docs = material.get("docs") or []
    question = payload.get("question") or {}
    sources = investigation.source_catalog(question, payload.get("sources"))
    maximum = max(0, min(20, int(payload.get("settings", {}).get("imageMax", 8))))
    limit = maximum if payload.get("supplement") else max(0, maximum - len(payload.get("existingImages") or []))
    if not limit:
        return {"images": []}
    existing = list(payload.get("existingImages") or [])
    folder = payload["imagesDir"]
    refs = payload.get("sourcePages") or []
    preferred = []
    if refs:
        for candidates in research._parallel(illustrations.read_source, refs[:8], 8, time.time() + 8):
            preferred.extend(candidates)
    if payload.get("sourceOnly"):
        for item in preferred:
            item["autoInsert"] = False
        return {"images": research.download_images(preferred[:32], folder, limit, time.time() + 12, existing)}

    if docs and not payload.get("supplement"):
        visual = (material.get("plan") or {}).get("images", [])
        phrases = [phrase for item in visual for phrase in item["queries"]]
        allowed_docs = [doc for doc in docs if doc.get("allowImage") or doc.get("sourceKind") in {"official", "wiki", "media", "community"}]
        preferred.extend(illustrations.source_candidates(allowed_docs, phrases))
    saved = research.download_images(preferred[:28], folder, limit, time.time() + 10, existing)
    if len(saved) >= limit:
        return {"images": saved}
    existing.extend(asset["filename"] for asset in saved)
    page = payload.get("publicPage")
    if page:
        candidates = illustrations.page_candidates(page)
    else:
        manual = investigation.strings(payload.get("imageQueries"), 3)
        if manual:
            visual = [{"subject": term, "queries": [term], "hosts": []} for term in manual]
        else:
            plan = material.get("plan") or investigation.plan_question(question, payload.get("model") or {}, sources, images_only=True)
            visual = plan["images"]
        candidates = illustrations.find_candidates(visual, sources, time.time() + 15)
    ranked, seen = [], set()
    for item in candidates:
        key = research.canonical_url(item["url"])
        if key not in seen:
            ranked.append(item)
            seen.add(key)
    return {"images": saved + research.download_images(ranked[:32], folder, limit - len(saved),
                                                        time.time() + 10, existing)}


# ---------------------------------------------------------------- HTTP

class Handler(BaseHTTPRequestHandler):
    server_version = f"ZhihuQAAgent/{VERSION}"

    def _send(self, code: int, obj: dict):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, fmt, *args):
        sys.stderr.write("[agent] " + (fmt % args) + "\n")

    def do_GET(self):
        if self.path.rstrip("/") in ("/health", "/"):
            self._send(
                200,
                {
                    "ok": True,
                    "version": VERSION,
                    "providers": {k: {"label": v["label"], "baseUrl": v["baseUrl"], "models": v["models"]}
                                  for k, v in llm.PROVIDERS.items()},
                },
            )
        elif self.path.rstrip("/") == "/v1/catalog":
            try:
                self._send(200, {"ok": True, **llm.sync_catalog()})
            except llm.LLMError as e:
                self._send(200, {"ok": False, "error": str(e)})
        else:
            self._send(404, {"ok": False, "error": "not found"})

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        try:
            payload = json.loads(self.rfile.read(length).decode("utf-8")) if length else {}
        except json.JSONDecodeError:
            self._send(400, {"ok": False, "error": "请求体不是合法 JSON"})
            return

        route = self.path.rstrip("/")
        try:
            if route == "/v1/test":
                self._send(200, llm.test_model(payload))
            elif route == "/v1/profile":
                self._send(200, do_profile(payload))
            elif route == "/v1/sources":
                self._send(200, do_sources(payload))
            elif route == "/v1/models":
                self._send(200, llm.list_models(payload))
            elif route == "/v1/research/collect":
                self._send(200, collect_material(payload))
            elif route == "/v1/research/qualify":
                self._send(200, qualify_material(payload))
            elif route == "/v1/research/draft":
                self._send(200, write_draft(payload))
            elif route == "/v1/research/review":
                self._send(200, review_material(payload))
            elif route == "/v1/research/images":
                self._send(200, collect_images(payload))
            elif route == "/v1/research/images/import":
                self._send(200, illustrations.import_image(payload))
            else:
                self._send(404, {"ok": False, "error": "not found"})
        except llm.LLMError as e:
            self._send(200, {"ok": False, "error": str(e)})
        except Exception as e:  # 兜底：把栈打到 stderr，前端拿到一句话
            traceback.print_exc()
            self._send(200, {"ok": False, "error": f"{type(e).__name__}: {e}"})


def watch_parent(pid: int) -> None:
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.DWORD)
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        kernel.CloseHandle.argtypes = (wintypes.HANDLE,)
        handle = kernel.OpenProcess(0x00100000, False, pid)
        if not handle:
            os._exit(0)
        kernel.WaitForSingleObject(handle, 0xffffffff)
        kernel.CloseHandle(handle)
        os._exit(0)
    while os.getppid() == pid:
        time.sleep(1)
    os._exit(0)


def main():
    global DATA_DIR
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8788)
    ap.add_argument("--data-dir", default=DATA_DIR)
    ap.add_argument("--parent-pid", type=int)
    args = ap.parse_args()
    if args.parent_pid:
        threading.Thread(target=watch_parent, args=(args.parent_pid,), daemon=True).start()
    DATA_DIR = args.data_dir
    os.makedirs(DATA_DIR, exist_ok=True)
    zhihu_open_search.load_local_secret(DATA_DIR)

    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    sys.stderr.write(f"[agent] 智能体已启动 → http://127.0.0.1:{args.port}（data: {DATA_DIR}）\n")
    sys.stderr.flush()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
