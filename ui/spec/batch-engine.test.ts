import { expect, test } from "vitest";
import { parseBatch } from "../src/batch/engine";

test("routes all AV input prefixes without changing the identifier", () => {
  const examples = [
    ["有码番号：", "ABC-123"],
    ["无码番号: ", "010124-123"],
    ["素人番号：", "300MIUM-001"],
    ["ｆｃ２番号 ： ", "FC2 PPV 1234567"],
    ["欧美番号：", "Brazzers.24.01.31"],
    ["国产番号：", "MD-123"],
    ["动漫里番：", "RJ123456"],
    ["MyWife: ", "Mywife-0123"],
    ["av: ", "ABC-456"],
    ["番号：", "ABC-789"],
  ];
  const input = ["电影：寄生虫", "剧集：Dark", ...examples.map(([label, id]) => label + id), "tmdb:550"];
  expect(parseBatch(input.join("\r\n"), "movie")).toEqual([
    { kind: "movie", query: "寄生虫" },
    { kind: "series", query: "Dark" },
    ...examples.map(([, query]) => ({ kind: "av", query })),
    { kind: "movie", query: "tmdb:550" },
  ]);
});
