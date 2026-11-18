import io
import json
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from PIL import Image, ImageDraw

import llm
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

    def test_actual_style_examples_and_citation_allowlist(self):
        html = "<p>我先放结果。" + "用最低限度的步骤跑一次。" * 16 + "</p><p>再说为什么。</p>"
        raw = [{"kind": "answer", "title": "铁路信号", "text": html}]
        examples = main._retain_examples(raw * 2)
        self.assertEqual(len(examples), 1)
        self.assertIn("\n\n再说为什么。", examples[0]["text"])
        self.assertNotIn("<p>", examples[0]["text"])
        doc = {"title": "手册", "sourceName": "官方", "sourceUrl": "https://manual.example/doc", "text": "铁路信号示意"}
        with patch.object(llm, "chat_json", return_value={
            "markdown": "正文。\n\n[IMG:fake.png]\n\n![随机图](https://other.example/image.png)",
            "citations": [{"url": "https://invented.example"}, {"url": doc["sourceUrl"]}],
            "doubts": [{"text": "信号是否默认开启"}],
        }) as call:
            output = main.write_draft({"question": {"title": "铁路信号"}, "profile": {"samples": examples}, "material": {"docs": [doc]}})
        self.assertIn("我先放结果。", call.call_args.args[1][1]["content"])
        self.assertEqual(output["draft"]["markdown"], "正文。")
        self.assertEqual([citation["url"] for citation in output["draft"]["citations"]], [doc["sourceUrl"]])
        self.assertEqual(output["draft"]["doubts"][0]["status"], "unverified")

    def test_no_evidence_is_not_verification(self):
        doubt = {"text": "最大长度", "status": "unverified"}
        material = {"docs": [{"sourceName": "手册", "title": "手册", "sourceUrl": "https://manual.example/doc", "text": "默认最大长度为十二节车厢。"}]}
        with patch.object(llm, "chat_json", return_value={"results": [{"index": 1, "status": "verified", "verdict": "确认", "quote": "未经资料支持的说法"}]}):
            result = main.review_material({"model": {}, "material": material, "doubts": [doubt]})
        self.assertEqual(result["doubts"][0]["status"], "unverified")

    def test_truncated_draft_salvages_without_second_request(self):
        with patch.object(llm, "chat", return_value='{"markdown":"' + "可以保留的草稿。" * 20) as call:
            output = llm.chat_json({}, [], retry_tokens=0, min_chars=40)
        self.assertTrue(output["_salvaged"])
        self.assertEqual(call.call_count, 1)


if __name__ == "__main__":
    unittest.main()
