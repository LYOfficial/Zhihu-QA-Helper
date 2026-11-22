import base64
import io
import json
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image, ImageDraw

import llm
import illustrations
import investigation
import main
import research


def illustration(variant="PNG", size=400, alternate=False):
    canvas = Image.new("RGB", (400, 320), "white")
    draw = ImageDraw.Draw(canvas)
    if alternate:
        draw.ellipse((78, 34, 210, 274), fill="#163849")
        draw.line((23, 153, 378, 126), fill="#ea7b12", width=30)
    else:
        draw.rectangle((23, 35, 182, 258), fill="#357eae")
        draw.polygon([(40, 270), (211, 50), (386, 285)], fill="#26292e")
    data = io.BytesIO()
    canvas.resize((size, size * 4 // 5)).save(data, format=variant)
    return data.getvalue()


class ResearchTests(unittest.TestCase):
    def test_community_images_use_public_body_originals_only(self):
        page = "https://www.zhihu.com/question/123/answer/456"
        html = '''<title>铁路信号</title><main><img src="https://pic1.zhimg.com/site.jpg">
        <div class="RichText"><img alt="区段示意图" data-original="https://pic1.zhimg.com/signal_r.jpg"
        src="https://pic1.zhimg.com/signal_b.jpg"><img src="https://pic1.zhimg.com/rail.jpg?watermark=1">
        <img src="https://zhimg.com.invalid/signal.png"><img src="http://127.0.0.1/image.png"></div></main>'''
        with patch.object(research, "fetch", return_value=(html.encode(), "text/html", "utf-8")):
            assets = illustrations.page_candidates(page)
        self.assertEqual([asset["url"] for asset in assets], ["https://pic1.zhimg.com/signal_r.jpg"])
        self.assertEqual(assets[0]["page"], page)
        self.assertFalse(assets[0]["allowed"]("https://127.0.0.1/signal.jpg"))
        with self.assertRaises(ValueError):
            illustrations.page_candidates("https://zhihu.com.invalid/question/123")

    def test_bing_images_keep_original_title_source_and_relevance(self):
        from html import escape
        source = "https://pixabay.com/photos/steam-locomotive-123/"
        results = [
            {"purl": source, "murl": "https://cdn.pixabay.com/steam.jpg", "t": "Steam locomotive at dusk"},
            {"purl": source, "murl": "https://cdn.pixabay.com/engine.jpg", "t": "Steam account registration"},
            {"purl": source, "murl": "https://th.bing.com/thumbnail.jpg", "t": "Steam locomotive"},
        ]
        markup = "".join(f'<a class="iusc" m="{escape(json.dumps(result))}"></a>' for result in results)
        with patch.object(research, "fetch", return_value=(markup.encode(), "text/html", "utf-8")):
            assets, _ = illustrations.bing_image_candidates("steam locomotive")
        self.assertEqual([(asset["sourceName"], asset["page"]) for asset in assets], [("Pixabay", source)])

    def test_import_dedup_survives_separate_requests(self):
        with tempfile.TemporaryDirectory() as folder:
            payload = {"imagesDir": folder, "imageData": base64.b64encode(illustration()).decode(),
                       "sourceUrl": "https://example.com/article", "caption": "铁路信号"}
            asset, = illustrations.import_image(payload)["images"]
            payload["existingImages"] = [asset["filename"]]
            payload["imageData"] = base64.b64encode(illustration("JPEG", size=370)).decode()
            self.assertEqual(illustrations.import_image(payload)["images"], [])
            self.assertEqual(len(list(Path(folder).glob("*.png"))), 1)

    def test_svg_title_does_not_swallow_article(self):
        body = "The off-hand can hold a torch in this preview. " * 7
        title, text, images = research.parse_doc(
            f'<title>Preview release</title><svg><title>Menu icon</title></svg>'
            f'<div>Outside the article</div><article><p>{body}</p><img src="/off-hand.jpg" alt="Off-hand torch"></article>',
            "https://manual.example/preview",
        )
        self.assertEqual(title, "Preview release")
        self.assertIn("off-hand can hold a torch", text)
        self.assertNotIn("Outside the article", text)
        self.assertEqual(len(images), 1)

    def test_help_center_search_uses_article_body_and_explicit_image_permission(self):
        endpoint = "https://manual.example/hc/en-us/articles/preview"
        entry = {"html_url": endpoint, "title": "Preview release", "body":
                 '<p>' + "The off-hand can hold a torch. " * 8 + '</p><img src="/off-hand.jpg" alt="Off-hand torch">'}
        other = {**entry, "html_url": "https://manual.example/hc/other/preview"}
        source = {"name": "Manual", "baseUrl": "https://manual.example/hc/en-us", "useFor": ["fact"]}
        raw = json.dumps({"results": [other, entry]}).encode()
        with patch.object(research, "fetch", return_value=(raw, "application/json", "utf-8")) as fetch:
            docs = research.gather('Minecraft "off-hand"', [source])
            self.assertEqual([doc["sourceUrl"] for doc in docs], [endpoint])
            self.assertIn("query=%22off-hand%22", fetch.call_args.args[0])
            self.assertFalse(research.pick_image_urls(docs, 3, "off-hand"))
            docs = research.gather('Minecraft "off-hand"', [{**source, "useFor": ["fact", "image"]}])
            self.assertEqual(len(research.pick_image_urls(docs, 3, "off-hand")), 1)

    def test_relevant_images_only_and_canonical_url(self):
        title, _, images = research.parse_doc('''<title>铁路信号教程</title>
            <header><img src="/railway-header.png" alt="铁路信号"></header>
            <article><img src="/blank.png" data-src="/signals.png?w=300" alt="铁路信号布局">
            <aside><img src="/ad.png" alt="铁路信号"></aside>
            <img src="/signals.png?w=500" alt="铁路信号布局"><img src="/signal-icon.png" alt="铁路信号">
            <img src="/stamp.svg" alt="铁路信号"><img src="/tiny.png" width="12" alt="铁路信号"></article>''',
            "https://manual.example/railway")
        doc = {"sourceUrl": "https://manual.example/railway", "sourceName": "手册", "title": title,
               "images": images, "authority": True, "allowImage": True}
        candidates = research.pick_image_urls([doc], 8, "铁路信号怎么看？")
        self.assertEqual([item["url"] for item in candidates], ["https://manual.example/signals.png?w=300"])
        self.assertEqual(research.pick_image_urls([{**doc, "authority": False}], 8, "铁路信号"), [])
        self.assertEqual(research.pick_image_urls([doc], 8, "电池容量"), [])

    def test_pixel_dedup_reencoding_rejects_corrupt_and_placeholder(self):
        original = illustration()
        variants = [original, original, illustration("JPEG", 220), b"<svg/>",
                    illustration(size=70), illustration(alternate=True)]
        urls = [f"https://manual.example/{index}" for index in range(len(variants))]
        candidates = [{"url": url, "page": "https://manual.example/manual", "sourceName": "手册", "caption": "示意"} for url in urls]
        with tempfile.TemporaryDirectory() as directory, patch.object(
            research, "fetch", side_effect=lambda url, **kwargs: (variants[urls.index(url)], "image/png", None),
        ):
            saved = research.download_images(candidates, directory, deadline=time.time() + 3)
            self.assertEqual([image["imageUrl"] for image in saved], [urls[0], urls[-1]])
            self.assertEqual({file.name for file in Path(directory).iterdir()}, {image["filename"] for image in saved})
            self.assertTrue(all(image["sourceUrl"].endswith("/manual") for image in saved))

    def test_draft_and_editor_receive_separate_context(self):
        html = "<p>我先放结果。" + "用最低限度的步骤跑一次。" * 16 + "</p><p>再说为什么。</p>"
        raw = [{"kind": "answer", "title": "铁路信号", "text": html}]
        examples = main._retain_examples(raw * 2)
        self.assertEqual(len(examples), 1)
        self.assertIn("\n\n再说为什么。", examples[0]["text"])
        self.assertNotIn("<p>", examples[0]["text"])
        doc = {"title": "手册", "sourceName": "官方", "sourceUrl": "https://manual.example/doc", "text": "铁路信号示意"}
        with patch.object(llm, "chat_json", side_effect=[{
            "markdown": "这是有关铁路的说明。信号用于显示区段状态。[[S1]][[S99]]",
            "citations": [{"url": "https://invented.example"}, {"id": "S1", "snippet": "说明信号用途"}],
            "doubts": [{"text": "信号是否默认开启"}],
        }, {"markdown": "铁路信号显示区段状态。[[S1]][[S99]]\n\n[IMG:fake.png]\n\n![随机图](https://other.example/image.png)"}]) as call:
            output = main.write_draft({
                "question": {"title": "铁路信号"}, "direction": "先说明信号的用途。",
                "profile": {"samples": examples, "style": {
                    "signature": ["口头禅哨兵"], "variants": ["教程先给可复现步骤"],
                }},
                "material": {"docs": [doc]},
            })
        draft_prompt, editor_prompt = [entry.args[1][1]["content"] for entry in call.call_args_list]
        self.assertIn(doc["sourceUrl"], draft_prompt)
        self.assertNotIn(doc["sourceUrl"], editor_prompt)
        self.assertIn("先说明信号的用途。", draft_prompt)
        self.assertIn("先说明信号的用途。", editor_prompt)
        self.assertIn("教程先给可复现步骤", editor_prompt)
        self.assertNotIn("教程先给可复现步骤", draft_prompt)
        for prompt in (draft_prompt, editor_prompt):
            self.assertNotIn("我先放结果。", prompt)
            self.assertNotIn("口头禅哨兵", prompt)
        self.assertEqual(output["draft"]["markdown"], "铁路信号显示区段状态。[1]\n\n### 参考资料\n\n[1] 手册 — https://manual.example/doc")
        self.assertEqual([citation["url"] for citation in output["draft"]["citations"]], [doc["sourceUrl"]])
        self.assertEqual(output["draft"]["doubts"][0]["status"], "unverified")
        self.assertEqual(output["warnings"], [])

    def test_no_evidence_is_not_verification(self):
        doubt = {"text": "最大长度", "status": "unverified"}
        material = {"docs": [{"sourceName": "手册", "title": "手册", "sourceUrl": "https://manual.example/doc",
                              "sourceKind": "official", "text": "默认最大长度为十二节车厢。"}]}
        response = {"results": [{"index": 1, "status": "verified", "verdict": "确认",
                                "evidence": [{"id": "S1", "stance": "supports", "quote": "未经资料支持的说法，不是在原文里。"}]}]}
        with patch.object(llm, "chat_json", return_value=response), patch.object(investigation, "gather_queries", return_value=[]) as search:
            result = main.review_material({"model": {"model": "test", "apiKey": "test"}, "material": material, "doubts": [doubt]})
        self.assertEqual(result["doubts"][0]["status"], "unverified")
        self.assertEqual(result["doubts"][0]["evidence"], [])
        self.assertEqual(search.call_args.args[0], ["最大长度"])

    def test_claim_follow_up_requires_source_id_quote_and_preserves_conflict(self):
        quote = "The throwing disc is coming in the first game drop of 2027."
        primary = {"sourceName": "Minecraft", "title": "Announcing throwing discs", "sourceUrl": "https://official.example/release",
                   "sourceKind": "official", "text": quote, "publishedAt": "2026-11-14"}
        rumor = {**primary, "title": "A player post", "sourceUrl": "https://example.org/post", "sourceKind": "community",
                 "text": "I thought the throwing disc was already released today."}
        claim = "飞盘已正式上线"
        initial = {"results": [{"index": 1, "status": "unverified", "queries": ["Minecraft throwing disc release date"]}]}
        expected = {"index": 1, "status": "corrected", "verdict": "公告说明计划于 2027 年首个更新推出",
                    "evidence": [{"id": "S1", "quote": quote, "stance": "refutes"}]}
        with patch.object(llm, "chat_json", side_effect=[initial, {"results": [expected]}]), \
                patch.object(investigation, "gather_queries", return_value=[primary]) as search:
            checks, _ = investigation.investigate_claims([claim], [rumor], [], {"model": "test", "apiKey": "test"})
        self.assertEqual(search.call_args.args[0], ["Minecraft throwing disc release date"])
        self.assertEqual(checks[0]["status"], "corrected")
        self.assertEqual(checks[0]["confidence"], 90)
        self.assertEqual(checks[0]["evidence"][0]["publishedAt"], "2026-11-14")
        expected["evidence"].append({"id": "S2", "quote": rumor["text"], "stance": "supports"})
        with patch.object(llm, "chat_json", return_value={"results": [expected]}):
            checks = investigation.verify([claim], [primary, rumor], {"model": "test", "apiKey": "test"})
        self.assertEqual(checks[0]["status"], "unverified")
        self.assertEqual(len(checks[0]["evidence"]), 2)

    def test_minecraft_image_plan_and_official_image_matching(self):
        question = {"title": "《我的世界》将新增回旋飞盘武器且基岩版解锁完整副手功能，会拓展出哪些新玩法？"}
        plan = investigation.plan_question(question, {}, [])
        self.assertEqual(len(plan["images"]), 2)
        self.assertTrue(all(question["title"] not in group["queries"] for group in plan["images"]))
        second = plan["images"][1]["queries"][1]
        self.assertTrue(illustrations.relevant(second, "A throwing disc in the off-hand"))
        self.assertFalse(illustrations.relevant(second, "A torch in the Bedrock off-hand"))
        self.assertFalse(illustrations.relevant(second, "Alexa on Amazon Bedrock"))

    def test_web_retrieval_runs_even_with_a_matching_directory(self):
        official = {"sourceUrl": "https://issuer.example/offhand", "sourceName": "公告", "sourceKind": "official",
                    "title": "发布副手功能", "text": "发布方公布副手功能当前仅限预览版。"}
        community = {**official, "sourceUrl": "https://tieba.baidu.com/p/121", "sourceKind": "community",
                     "text": "我在副手功能预览版发现这个情况。"}
        with patch.object(research, "gather", return_value=[official]), patch.object(research, "gather_web",
                side_effect=lambda *args, **opts: [community] if opts.get("lane") == "discussion" else []) as search:
            docs = investigation.gather_queries(["副手功能"], [], time.time() + 2, hotspot=True)
        self.assertTrue(any(call.kwargs.get("recent") for call in search.call_args_list))
        self.assertTrue(any(call.kwargs.get("lane") == "news" for call in search.call_args_list))
        self.assertEqual({doc["sourceKind"] for doc in docs}, {"official", "community"})

    def test_verification_uses_stage_budget_instead_of_old_short_timeout(self):
        material = {"docs": [{"sourceUrl": "https://example.com/doc", "sourceName": "手册", "title": "手册",
                              "text": "手册说明铁路信号的适用范围。"}]}
        payload = {"material": material, "model": {"model": "fixture", "apiKey": "fixture"},
                   "settings": {"verifyTimeoutSeconds": 120}, "doubts": [{"text": "核对适用范围", "status": "open"}]}
        with patch.object(investigation, "gather_queries", return_value=[]), \
                patch.object(llm, "chat_json", return_value={"results": []}) as model:
            main.review_material(payload)
        self.assertGreater(model.call_args.kwargs["timeout"], 100)
        self.assertLessEqual(model.call_args.kwargs["timeout"], 120)

    def test_source_gallery_reads_body_carousel_without_search_or_autoinsertion(self):
        page = "https://www.minecraft.net/en-us/article/example"
        html = '''<title>游戏更新</title><header><img src="/logo.png"></header>
        <div class="article-grid-a"><div class="MC_Carousel"><img data-src="/assets/disc.jpg" alt="投掷飞盘"></div>
        <img src="/placeholder.png" srcset="/small.jpg 320w, /assets/offhand.jpg 1280w" alt="副手"></div>'''
        with patch.object(research, "fetch", return_value=(html.encode(), "text/html", "utf-8")), \
                patch.object(research, "public_url", return_value=True), \
                patch.object(research, "download_images", return_value=[]) as download, \
                patch.object(illustrations, "find_candidates") as search:
            main.collect_images({"sourcePages": [{"url": page, "sourceName": "官网"}], "sourceOnly": True,
                                 "supplement": True, "imagesDir": "unused"})
        candidates = download.call_args.args[0]
        self.assertEqual([entry["url"].rsplit("/", 1)[-1] for entry in candidates], ["disc.jpg", "offhand.jpg"])
        self.assertTrue(all(entry["page"] == page and not entry["autoInsert"] for entry in candidates))
        search.assert_not_called()

    def test_editor_can_reorder_cited_facts_and_cut_repetition(self):
        draft = (
            "这里先来聊一下，为什么这个系统会这样设计。\n\n"
            "随身合成区是 2×2。[[S1]] 门口的工作台能提供 3×3 的格子。[[S2]]\n\n"
            "归根结底，工作台能提供 3×3 的格子。[[S2]]"
        )
        edited = "工作台有 3 × 3 格。[[S2]] 随身合成区只有 2 × 2 格。[[S1]]"
        docs = [
            {"title": title, "sourceName": "手册", "sourceUrl": f"https://manual.example/{index}", "text": title}
            for index, title in enumerate(("随身合成区", "工作台"), 1)
        ]
        with patch.object(llm, "chat_json", side_effect=[{"markdown": draft}, {"markdown": edited}]):
            output = main.write_draft({"material": {"docs": docs}})
        self.assertTrue(output["draft"]["markdown"].startswith("工作台有 3 × 3 格。[1] 随身合成区只有 2 × 2 格。[2]"))
        self.assertEqual([item["label"] for item in output["draft"]["citations"]], ["工作台", "随身合成区"])
        self.assertEqual(output["warnings"], [])

    def test_editor_falls_back_when_edit_fails_or_changes_protected_details(self):
        draft = "预览版 26.70.26 支持这项功能。[[S1]]\n\n运行 `ping`：\n\n~~~sh\n  ping localhost\n~~~"
        variants = [
            {"markdown": draft.replace("26.70.26", "26.70.27")},
            {"markdown": draft.replace("S1", "S2")},
            {"markdown": draft.replace("  ping", "ping")},
            {"markdown": draft.replace("`ping`", "`pong`")},
            {"markdown": draft, "_salvaged": True},
            llm.LLMError("模拟编辑请求超时"),
        ]
        doc = {"title": "预览版", "sourceName": "手册", "sourceUrl": "https://manual.example/preview", "text": "预览版"}
        for variant in variants:
            with self.subTest(variant=variant), patch.object(llm, "chat_json", side_effect=[{"markdown": draft}, variant]):
                output = main.write_draft({"material": {"docs": [doc]}})
                self.assertTrue(output["draft"]["markdown"].startswith(draft.replace("[[S1]]", "[1]")))
                self.assertIn("已保留第一版初稿", output["warnings"][0])

    def test_search_union_keeps_original_result_urls(self):
        html = '<a data-mdurl="https://www.cas.cn/article">月壤</a><a data-mdurl="https://ai.so.com/search">聚合摘要</a>'
        rss = '<rss><channel><item><link>https://www.mnr.gov.cn/article</link></item></channel></rss>'

        def response(url, **kwargs):
            data = html if "so.com" in url else rss if "format=rss" in url else ""
            return data.encode(), "text/html", "utf-8"

        with patch.object(research, "fetch", side_effect=response):
            self.assertEqual(research.web_search("嫦娥六号 月壤"), ["https://www.cas.cn/article", "https://www.mnr.gov.cn/article"])

    def test_truncated_draft_salvages_without_second_request(self):
        with patch.object(llm, "chat", return_value='{"markdown":"' + "可以保留的草稿。" * 20) as call:
            output = llm.chat_json({}, [], retry_tokens=0, min_chars=40)
        self.assertTrue(output["_salvaged"])
        self.assertEqual(call.call_count, 1)


if __name__ == "__main__":
    unittest.main()
