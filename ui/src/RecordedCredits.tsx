import { useState } from "react";
import { safeUrl } from "./Catalog";
import type { Fields, Source } from "./types";

type Role = "声优" | "演员" | "导演" | "编剧" | "制片人" | "其他幕后人员";
type Person = { role: Role; name: string; detail?: string; portrait?: string };
const roles: Role[] = ["声优", "演员", "导演", "编剧", "制片人", "其他幕后人员"];
const fieldsByRole: Partial<Record<Role, string[]>> = { 声优: ["voice_actors"], 演员: ["cast", "actors", "actresses", "other_cast"], 导演: ["directors"], 编剧: ["writers"], 制片人: ["producers"] };
const record = (value: unknown): Record<string, unknown> => value && typeof value === "object" ? value as Record<string, unknown> : {};
const list = (value: unknown) => Array.isArray(value) ? value.map(record) : [];
const text = (value: unknown) => typeof value === "string" && value.trim() ? value.trim() : undefined;
const voice = (value?: string) => value && /\(voice|声优|声優|配音/i.test(value);
function job(name?: string): Role {
  if (name && /^(导演|监督)$/.test(name)) return "导演";
  if (name && /^(脚本|编剧|系列构成)$/.test(name)) return "编剧";
  if (name && /^(制片人|执行制片人)$/.test(name)) return "制片人";
  if (name && /\bdirector\b/i.test(name) && !/art director|casting director|director of photography/i.test(name)) return "导演";
  if (name && /writer|screenplay|screenwriter|series composition|teleplay/i.test(name)) return "编剧";
  if (name && /producer/i.test(name)) return "制片人";
  return "其他幕后人员";
}

function collect(sources: Source[], fields: Fields) {
  const people = new Map<string, Person>();
  function add(role: Role, name?: string, detail?: string, portrait?: string) {
    if (!name) return;
    const key = role + ":" + name.toLowerCase();
    const previous = people.get(key);
    people.set(key, { role, name: previous?.name ?? name, detail: previous?.detail ?? detail, portrait: previous?.portrait ?? portrait });
  }
  for (const source of sources) {
    const data = record(source.data);
    if (source.provider === "TMDB") {
      const animated = list(data.genres).some((genre) => genre.id === 16);
      const credits = record(data.aggregate_credits ?? data.credits);
      function image(person: Record<string, unknown>) {
        const profile = text(person.profile_path);
        return profile && /^\/[\w-]+\.(jpg|jpeg|png|webp)$/i.test(profile) ? `https://image.tmdb.org/t/p/w185${profile}` : undefined;
      }
      for (const person of list(credits.cast)) {
        const character = text(person.character) ?? list(person.roles).map((role) => text(role.character)).filter(Boolean).join(" / ");
        add(animated || voice(character) ? "声优" : "演员", text(person.name), character, image(person));
      }
      for (const person of list(credits.crew)) {
        const titles = [text(person.job), ...list(person.jobs).map((entry) => text(entry.job))].filter(Boolean);
        for (const title of new Set(titles)) add(job(title), text(person.name), title, image(person));
      }
    } else if (source.provider === "TVmaze") {
      const embedded = record(data._embedded);
      for (const kind of ["cast", "crew"] as const) for (const credit of list(embedded[kind])) {
        const person = record(credit.person);
        const image = record(person.image);
        const path = safeUrl(text(image.medium) ?? text(image.original));
        const detail = kind === "cast" ? text(record(credit.character).name) : text(credit.type);
        add(kind === "crew" ? job(detail) : credit.voice === true ? "声优" : "演员", text(person.name), detail,
          path && new URL(path).hostname === "static.tvmaze.com" ? path : undefined);
      }
    } else if (source.provider === "Bangumi") {
      const actorRole = record(data.subject).type === 2 ? "声优" : "演员";
      const image = (person: Record<string, unknown>) => {
        const images = record(person.images);
        const path = safeUrl(text(images.medium) ?? text(images.large));
        return path && new URL(path).hostname === "lain.bgm.tv" ? path : undefined;
      };
      for (const character of list(data.characters)) for (const actor of list(character.actors)) {
        add(actorRole, text(actor.name), text(character.name), image(actor));
      }
      for (const person of list(data.persons)) {
        if (person.type !== 1) continue;
        const detail = text(person.relation);
        const role = actorRole === "演员" && detail && /^(主演|演员|配角)$/.test(detail) ? "演员" : job(detail);
        add(role, text(person.name), detail, image(person));
      }
    }
  }
  for (const role of roles) for (const key of fieldsByRole[role] ?? []) {
    const values = fields[key];
    if (!Array.isArray(values)) continue;
    for (const value of values) {
      const name = text(value);
      if (role === "演员" && name && people.has("声优:" + name.toLowerCase())) continue;
      add(role, name);
    }
  }
  return [...people.values()];
}

function Portrait({ src }: { src?: string }) {
  const [failed, setFailed] = useState(false);
  return <span className="credit-art" aria-hidden="true">
    <span className="poster-placeholder">无头像</span>
    {src && !failed && <img src={src} alt="" loading="lazy" decoding="async" referrerPolicy="no-referrer" onError={() => setFailed(true)} />}
  </span>;
}

export function RecordedCredits({ sources, fields }: { sources: Source[]; fields: Fields }) {
  const people = collect(sources, fields);
  if (!people.length) return <p className="work-panel muted">来源未提供演职员资料。</p>;
  return <div className="work-panel grid gap-5">{roles.map((role) => {
    const group = people.filter((person) => person.role === role);
    return group.length > 0 && <section key={role} aria-label={role}>
      <h3 className="mb-2 text-base">{role}</h3>
      <ul className="credit-grid grid grid-cols-credits gap-2.5 sm:grid-cols-credits-wide sm:gap-3" role="list">{group.map(({ name, detail, portrait }) => <li key={name}>
        <Portrait src={portrait} /><strong>{name}</strong>{detail && <span>{detail}</span>}
      </li>)}</ul>
    </section>;
  })}</div>;
}
