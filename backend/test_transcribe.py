import unittest
from types import SimpleNamespace

from backend.transcribe import correct_timed_words, timed_words, transcription_terms, trim_overlap


class CorrectionTimingTests(unittest.TestCase):
    def align(self, parts, rules, offset=0):
        text = "".join(parts)
        words = [
            SimpleNamespace(word=part, start=index * 2, end=index * 2 + 1)
            for index, part in enumerate(parts)
        ]
        aligned = timed_words(words, offset, offset, offset + len(parts) * 2, text, str)
        _, pattern, replacements = transcription_terms({
            "terms": [{"from": before, "to": after} for before, after in rules]
        })
        return correct_timed_words(text, aligned, pattern, replacements)

    def test_preserves_unaffected_model_times(self):
        text, words = self.align(["欢迎", "使用", "套瑞"], [("套瑞", "Tauri")])
        self.assertEqual(text, "欢迎使用Tauri")
        self.assertEqual(words, [
            {"text": "欢迎", "startSec": 0, "endSec": 1},
            {"text": "使用", "startSec": 2, "endSec": 3},
            {"text": "Tauri", "startSec": 4, "endSec": 5, "estimated": True},
        ])

    def test_partial_and_multi_word_replacements_keep_absolute_times(self):
        text, words = self.align(["甲套", "瑞乙", "尾"], [("套瑞", "Tauri"), ("尾", "终")], offset=10)
        self.assertEqual(text, "甲Tauri乙终")
        self.assertEqual([(word["startSec"], word["endSec"]) for word in words],
                         [(10, 10.5), (10.5, 12.5), (12.5, 13), (14, 15)])
        self.assertTrue(all(word["estimated"] for word in words))

    def test_replacements_work_without_model_timing(self):
        _, pattern, replacements = transcription_terms({"terms": [{"from": "套瑞", "to": "Tauri"}]})
        self.assertEqual(correct_timed_words("套瑞", [], pattern, replacements), ("Tauri", []))

    def test_rules_without_a_match_do_not_change_word_alignment(self):
        text, words = self.align(["欢迎", "使用"], [("套瑞", "Tauri")])
        self.assertEqual(text, "欢迎使用")
        self.assertEqual(words, [
            {"text": "欢迎", "startSec": 0, "endSec": 1},
            {"text": "使用", "startSec": 2, "endSec": 3},
        ])

    def test_resume_matches_corrected_checkpoint_text(self):
        _, pattern, replacements = transcription_terms({"terms": [{"from": "套瑞", "to": "Tauri"}]})
        words = [
            SimpleNamespace(word="套", start=0, end=0.2),
            SimpleNamespace(word="瑞", start=0.2, end=0.8),
            SimpleNamespace(word="测试", start=0.8, end=1.4),
        ]
        segment = SimpleNamespace(start=0, end=1.4, text="套瑞测试", words=words)

        def correct(text):
            return correct_timed_words(text, [], pattern, replacements)[0]

        self.assertEqual(trim_overlap(segment, 10, 10.4, "欢迎使用Tauri", correct),
                         (10.8, 11.4, "测试", words[2:]))
