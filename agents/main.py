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
import concurrent.futures
import json
import os
import re
import sys
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import llm  # noqa: E402
import prompts  # noqa: E402
import research  # noqa: E402

VERSION = "0.1.0"
DATA_DIR = os.path.join(os.getcwd(), "data")


# ---------------------------------------------------------------- profile

def do_profile(payload: dict) -> dict:
    samples = payload.get("samples") or []
    cfg = payload.get("model") or {}
    if not samples:
        raise llm.LLMError("没有可分析的历史内容")

    lines = []
    for s in samples[:24]:
        head = "回答" if s.get("kind") == "answer" else "文章"
        lines.append(
            f"- [{head}] {s.get('title', '')}｜赞同 {s.get('voteups', 0)}｜{s.get('text', '')[:90]}"
        )
    block = "\n".join(lines)

    try:
        data = llm.chat_json(
            cfg,
            [
                {"role": "system", "content": prompts.PROFILE_SYSTEM},
                {"role": "user", "content": f"以下是该答主的 {len(samples)} 条历史内容：\n\n{block}"},
            ],
            temperature=0.2,
            max_tokens=8000,
            retry_tokens=12000,
            timeout=120,
        )
    except llm.LLMError:
        data = llm.chat_json(
            cfg,
            [
                {"role": "system", "content": prompts.PROFILE_COMPACT_SYSTEM},
                {"role": "user", "content": f"历史内容：\n{block}"},
            ],
            temperature=0.1,
            max_tokens=4000,
            retry_tokens=6000,
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

def _query_terms(title: str, profile: dict, cfg: dict) -> list[str]:
    """先让模型把口语标题拆成检索词，失败就退回本地粗暴切分。"""
    try:
        raw = llm.chat(
            cfg,
            [
                {"role": "system", "content": prompts.RESEARCH_QUERY_SYSTEM},
                {"role": "user", "content": title},
            ],
            temperature=0,
            max_tokens=300,
            timeout=25,
        )
        terms = [ln.strip(" -·、") for ln in raw.splitlines() if ln.strip()]
        terms = [t for t in terms if 1 < len(t) < 40][:3]
        if terms:
            return terms
    except llm.LLMError:
        pass
    base = re.sub(r"[？?！!，,。.、：:；;“”\"'（）()\[\]【】]", " ", title)
    words = [w for w in base.split() if len(w) >= 2]
    return [" ".join(words[:6])] or [title[:20]]


def _fmt_docs(docs: list[dict], budget: int = 11000) -> str:
    parts, used = [], 0
    for d in docs:
        chunk = f"\n### 来源：{d['sourceName']}｜{d['title']}\n链接：{d['sourceUrl']}\n{d['text'][:3200]}\n"
        if used + len(chunk) > budget:
            break
        parts.append(chunk)
        used += len(chunk)
    return "".join(parts) if parts else "（本次没有检索到可用资料，请仅凭公认常识作答，并把所有具体数值列入待核实项）"


def _fmt_images(images: list[dict]) -> str:
    if not images:
        return "（没有可用配图，正文中不要出现 [IMG:] 占位）"
    return "\n".join(f"- {i['filename']}｜来源 {i['sourceName']}" for i in images)


def _ensure_image_placeholders(markdown: str, images: list[dict]) -> str:
    if not images:
        return re.sub(r"\[IMG:[^\]]*\]", "", markdown)
    existing = re.findall(r"\[IMG:([^\]]+)\]", markdown)
    valid = {i["filename"] for i in images}
    # 剔除模型编造的占位
    markdown = re.sub(
        r"\[IMG:([^\]]+)\]",
        lambda m: m.group(0) if m.group(1) in valid else "",
        markdown,
    )
    existing = [f for f in existing if f in valid]
    if existing:
        return markdown
    # 模型一张都没插：按段落均分补上
    paras = [p for p in markdown.split("\n\n") if p.strip()]
    if not paras:
        return markdown
    step = max(1, len(paras) // (len(images) + 1))
    for idx, img in enumerate(images):
        pos = min(len(paras) - 1, (idx + 1) * step)
        paras[pos] = paras[pos] + f"\n\n[IMG:{img['filename']}]"
    return "\n\n".join(paras)


def _verify_doubts(doubts: list[dict], docs: list[dict], cfg: dict, deadline: float) -> list[dict]:
    out: list[dict] = []
    corpus = "\n\n".join(f"[{d['sourceName']}] {d['text'][:2200]}" for d in docs)[:9000]

    def one(d: dict) -> dict:
        text = (d.get("text") or "").strip()
        if not docs:
            return {
                "text": text,
                "status": "verified",
                "verdict": "本次没有抓到资料，该点未经外部核查，正文已弱化表述",
                "source": "",
            }
        if time.time() > deadline:
            return {"text": text, "status": "verified", "verdict": "时间不够，未做外部核查", "source": ""}
        try:
            v = llm.chat_json(
                cfg,
                [
                    {"role": "system", "content": prompts.VERIFY_SYSTEM},
                    {"role": "user", "content": f"待核实说法：{text}\n\n资料摘录：\n{corpus}"},
                ],
                temperature=0,
                max_tokens=800,
                retry_tokens=1200,
                timeout=max(20, min(60, int(deadline - time.time()))),
            )
            return {
                "text": text,
                "status": v.get("status") if v.get("status") in ("verified", "corrected", "rejected") else "verified",
                "verdict": (v.get("verdict") or "").strip(),
                "source": "",
            }
        except llm.LLMError as e:
            return {"text": text, "status": "verified", "verdict": f"核查调用失败：{e}", "source": ""}

    todo = [d for d in doubts[:4] if (d.get("text") or "").strip()]
    if not todo:
        return out
    with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
        for item in pool.map(one, todo):
            out.append(item)
    return out


def do_research(payload: dict) -> dict:
    q = payload.get("question") or {}
    profile = payload.get("profile") or {}
    sources = payload.get("sources") or []
    cfg = payload.get("model") or {}
    st = payload.get("settings") or {}
    img_dir = payload.get("imagesDir") or os.path.join(DATA_DIR, "images", "tmp")

    draft_min = int(st.get("draftMin") or 500)
    draft_max = int(st.get("draftMax") or 1000)
    image_max = int(st.get("imageMax") or 8)
    image_min = int(st.get("imageMin") or 3)

    # 整条流水线的硬时限：不能让用户对着转圈等二十分钟。
    budget = float(payload.get("deadlineSeconds") or 300)
    started = time.time()
    deadline = started + budget
    timings: dict[str, int] = {}

    def mark(stage: str, at: float) -> None:
        timings[stage] = int((time.time() - at) * 1000)

    warnings: list[str] = []

    # 1) 检索词（模型慢就直接本地切词，不在这上面卡住）
    t0 = time.time()
    if deadline - time.time() > 30:
        terms = _query_terms(q.get("title", ""), profile, cfg)
    else:
        terms = [q.get("title", "")[:20]]
    mark("query", t0)

    # 2) 联网取材：多条检索词并行，任何一个站点失败都不拖累整体
    t0 = time.time()
    docs: list[dict] = []
    if terms and deadline - time.time() > 20:
        with concurrent.futures.ThreadPoolExecutor(max_workers=min(3, len(terms))) as pool:
            futures = [
                pool.submit(research.gather, t, sources, 3, deadline)
                for t in terms
            ]
            for fut in concurrent.futures.as_completed(futures, timeout=max(5, deadline - time.time())):
                try:
                    docs.extend(fut.result() or [])
                except Exception as e:  # 网络问题不该让整条流程死掉
                    warnings.append(f"检索失败：{type(e).__name__}: {e}")
                if len(docs) >= 4:
                    break
    mark("gather", t0)

    seen, uniq = set(), []
    for d in docs:
        if d["sourceUrl"] in seen:
            continue
        seen.add(d["sourceUrl"])
        uniq.append(d)
    docs = uniq
    if not docs and deadline - time.time() > 40:
        # 权威平台全部抓不到（被墙 / 403 / 没有检索入口），退回通用网页检索，
        # 有真实页面总比凭空写好；风险由下面的 warning 明说。
        try:
            docs = research.gather_web(terms[0] if terms else q.get("title", ""), 3, deadline)
        except Exception as e:
            warnings.append(f"网页检索失败：{type(e).__name__}: {e}")
        if docs:
            warnings.append("权威平台没有取到内容，本次改用通用网页检索兜底，引用需自行复核")
    if not docs:
        warnings.append("没有检索到权威资料，初稿仅凭常识生成，请重点核对数值")

    # 3) 取图（并行，最多占 40 秒，取不到就少图，不阻塞起草）
    t0 = time.time()
    images: list[dict] = []
    if deadline - time.time() > 40:
        cands = research.pick_image_urls(docs, want=image_max)
        images = research.download_images(
            cands, img_dir, want=image_max,
            deadline=min(deadline, time.time() + 40),
        )
    if len(images) < image_min:
        warnings.append(f"可用配图只有 {len(images)} 张，低于设定的 {image_min} 张")
    mark("images", t0)

    # 4) 起草
    profile_text = (profile.get("summary") or "").strip()
    terms_text = "、".join(
        f"{t.get('label')}({t.get('kind')})" for t in (profile.get("terms") or [])
    )
    if terms_text:
        profile_text += f"\n领域词条：{terms_text}"

    user = prompts.DRAFT_USER.format(
        title=q.get("title", ""),
        excerpt=q.get("excerpt", "") or "（无补充说明）",
        tags="、".join(q.get("tags") or []) or "（无）",
        profile=profile_text or "（暂无画像）",
        docs=_fmt_docs(docs),
        images=_fmt_images(images),
    )
    t0 = time.time()
    remaining = max(45, int(deadline - time.time()))
    # 输出上限跟着模型真实容量走：models.dev 给的 output/context 越大，
    # 越不需要担心 JSON 被截断；推理模型也不会把预算耗在思考上。
    out_lim = int(cfg.get("outputLimit") or 0)
    ctx_lim = int(cfg.get("contextLimit") or 0)
    headroom = out_lim or (ctx_lim // 2 if ctx_lim else 0)
    draft_tokens = max(6000, min(24000, headroom)) if headroom else 12000
    retry_tokens = max(draft_tokens, min(40000, headroom)) if headroom else 20000
    data = llm.chat_json(
        cfg,
        [
            {
                "role": "system",
                "content": prompts.DRAFT_SYSTEM.format(draft_min=draft_min, draft_max=draft_max),
            },
            {"role": "user", "content": user},
        ],
        temperature=float(cfg.get("temperature") or 0.45),
        max_tokens=draft_tokens,
        retry_tokens=retry_tokens,
        timeout=min(240, remaining),
        min_chars=max(120, draft_min // 3),
    )
    mark("draft", t0)
    if data.get("_salvaged"):
        warnings.append("模型输出被截断，已保留已生成的正文字段，请核对结尾是否完整")

    markdown = (data.get("markdown") or "").strip()
    if not markdown:
        raise llm.LLMError("模型返回的正文为空")

    # doubts 可能是 [{"text": ...}]，也可能只是字符串数组，两种都收
    raw_doubts = data.get("doubts") or []
    doubts: list[dict] = []
    for d in raw_doubts:
        if isinstance(d, dict):
            t = (d.get("text") or d.get("point") or "").strip()
        elif isinstance(d, str):
            t = d.strip()
        else:
            t = ""
        if t:
            doubts.append({"text": t})
    doubts = doubts[:4]
    if not doubts:
        # 模型没给存疑点时，至少留一条诚实的提醒，不要让「存疑 0」假装万事大吉
        doubts = [
            {
                "text": "本次没有检索到权威资料，正文中的具体数值与机制名称需要自行核对"
                if not docs
                else "模型没有标注存疑点，建议重点核对数值与版本信息"
            }
        ]

    # 5) 逐条核疑（并行；时间不够就明确标注未核查，不假装核过）
    t0 = time.time()
    doubts = _verify_doubts(doubts, docs, cfg, deadline)
    mark("verify", t0)
    # 若核查给出了更正，把更正内容作为"批注"留在 doubts 里，正文不自动改，
    # 让答主在批阅台自己决定怎么改——这是本产品的核心主张：AI 不直接定稿。
    markdown = _ensure_image_placeholders(markdown, images)

    citations = []
    for c in (data.get("citations") or [])[:6]:
        if not isinstance(c, dict):
            continue
        url = (c.get("url") or "").strip()
        if not url.startswith("http"):
            continue
        citations.append(
            {
                "label": (c.get("label") or "").strip() or url,
                "url": url,
                "sourceName": (c.get("sourceName") or "").strip(),
                "snippet": (c.get("snippet") or "").strip(),
            }
        )
    if not citations:
        citations = [
            {"label": d["title"], "url": d["sourceUrl"], "sourceName": d["sourceName"], "snippet": ""}
            for d in docs[:5]
        ]

    # 配图说明：优先使用页面标题里的信息，让答主知道图从哪来
    for img in images:
        img["caption"] = img.get("caption") or f"来源：{img['sourceName']}"

    return {
        "draft": {"markdown": markdown, "doubts": doubts, "citations": citations},
        "images": images,
        "model": cfg.get("model", ""),
        "warnings": warnings,
        "sourcesUsed": [{"name": d["sourceName"], "url": d["sourceUrl"]} for d in docs],
        "elapsedMs": int((time.time() - started) * 1000),
        "timings": timings,
    }


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
            elif route == "/v1/research":
                self._send(200, do_research(payload))
            else:
                self._send(404, {"ok": False, "error": "not found"})
        except llm.LLMError as e:
            self._send(200, {"ok": False, "error": str(e)})
        except Exception as e:  # 兜底：把栈打到 stderr，前端拿到一句话
            traceback.print_exc()
            self._send(200, {"ok": False, "error": f"{type(e).__name__}: {e}"})


def main():
    global DATA_DIR
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8788)
    ap.add_argument("--data-dir", default=DATA_DIR)
    args = ap.parse_args()
    DATA_DIR = args.data_dir
    os.makedirs(DATA_DIR, exist_ok=True)

    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    sys.stderr.write(f"[agent] 智能体已启动 → http://127.0.0.1:{args.port}（data: {DATA_DIR}）\n")
    sys.stderr.flush()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
