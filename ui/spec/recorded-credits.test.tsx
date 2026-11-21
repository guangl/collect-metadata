import { render, screen, within } from "@testing-library/react";
import { expect, test } from "vitest";
import { RecordedCredits } from "../src/RecordedCredits";
import type { Source } from "../src/types";

test("uses animation roles and available portraits from stored source data", () => {
  const source: Source = {
    provider: "TMDB", status: "ok", url: "https://www.themoviedb.org/tv/1", note: null, fields: {},
    data: { genres: [{ id: 16 }], aggregate_credits: {
      cast: [{ name: "声优甲", profile_path: "/profile.jpg", roles: [{ character: "主人公" }] }],
      crew: [{ name: "导演甲", profile_path: "/director.jpg", jobs: [{ job: "Series Director" }] }],
    } },
  };
  render(<RecordedCredits sources={[source]} fields={{ cast: ["声优甲"], writers: ["无照编剧"] }} />);
  const voice = within(screen.getByRole("region", { name: "声优" }));
  expect(voice.getByText("主人公")).toBeTruthy();
  expect(voice.getByText("声优甲").closest("li")?.querySelector("img")?.src).toBe("https://image.tmdb.org/t/p/w185/profile.jpg");
  expect(screen.queryByRole("region", { name: "演员" })).toBeNull();
  expect(within(screen.getByRole("region", { name: "导演" })).getByText("导演甲")).toBeTruthy();
  expect(screen.getByText("无照编剧").closest("li")?.querySelector("img")).toBeNull();
});
