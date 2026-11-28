import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const source = readFileSync(new URL("../ui/reading-position.js", import.meta.url), "utf8");
const { renderCharacterJson } = runInNewContext(source + "\n({renderCharacterJson});");

test("JSON retains model timing around an estimated single-character correction", () => {
  const record = {
    title: "Correction",
    language: "zh",
    segments: [{
      text: "甲B乙",
      startSec: 0,
      endSec: 3,
      words: [
        { text: "甲", startSec: 0, endSec: 0.5 },
        { text: "B", startSec: 1, endSec: 2, estimated: true },
        { text: "乙", startSec: 2.5, endSec: 3 },
      ],
    }],
  };
  const { characters } = JSON.parse(renderCharacterJson(record)).segments[0];
  assert.deepEqual(characters.map(({ startMs, endMs, timing }) => [startMs, endMs, timing]), [
    [0, 500, "whisper"],
    [1000, 2000, "word_interpolated"],
    [2500, 3000, "whisper"],
  ]);
});
