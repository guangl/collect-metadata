import { useEffect, useRef, useState } from "react";
import { isDesktop, loadEmbyImage, type EmbyImage } from "../../api";

const artworkClasses: Record<EmbyImage, string> = {
  poster: "poster-art",
  backdrop: "fanart-art",
  logo: "logo-art",
  landscape: "fanart-art",
  episode: "episode-art",
  portrait: "credit-art",
};

const artworkNames: Record<EmbyImage, string> = {
  poster: "海报",
  backdrop: "背景图",
  logo: "透明标题图",
  landscape: "横版缩略图",
  episode: "剧照",
  portrait: "头像",
};

const imageQueue: (() => Promise<void>)[] = [];
let loadingImages = 0;

function loadNextImage() {
  if (loadingImages >= 6) return;
  const task = imageQueue.shift();
  if (!task) return;
  loadingImages++;
  void task().finally(() => {
    loadingImages--;
    loadNextImage();
  });
}

export function EmbyArtwork({ id, tag, image, eager = false, alt }: {
  id: string;
  tag: string | null;
  image: EmbyImage;
  eager?: boolean;
  alt?: string;
}) {
  const host = useRef<HTMLSpanElement>(null);
  const imageKey = `${id}/${image}?v=${tag}`;
  const [local, setLocal] = useState<{ key: string; url?: string }>();
  const [loaded, setLoaded] = useState<string>();
  const [failed, setFailed] = useState<string>();
  const src = !tag ? undefined : isDesktop ? (local?.key === imageKey ? local.url : undefined)
    : `/api/emby/images/${encodeURIComponent(id)}/${image}?v=${encodeURIComponent(tag)}`;

  useEffect(() => {
    if (!isDesktop || !tag || !host.current) return;
    let stopped = false;
    let url: string | undefined;
    let observer: IntersectionObserver | undefined;
    async function load() {
      try {
        url = await loadEmbyImage(id, image);
        if (stopped) URL.revokeObjectURL(url);
        else setLocal({ key: imageKey, url });
      } catch {
        if (!stopped) setLocal({ key: imageKey });
      }
    }
    function enqueue() {
      if (eager) imageQueue.unshift(load);
      else imageQueue.push(load);
      loadNextImage();
    }

    if (eager || typeof IntersectionObserver === "undefined") {
      enqueue();
    } else {
      observer = new IntersectionObserver(([entry]) => {
        if (entry.isIntersecting) {
          observer?.disconnect();
          enqueue();
        }
      }, { rootMargin: "240px" });
      observer.observe(host.current);
    }
    return () => {
      stopped = true;
      observer?.disconnect();
      const queued = imageQueue.indexOf(load);
      if (queued >= 0) imageQueue.splice(queued, 1);
      if (url) URL.revokeObjectURL(url);
    };
  }, [id, image, tag, eager, imageKey]);

  const unavailable = (local?.key === imageKey && !local.url) || (!!src && failed === src);
  return <span ref={host} className={artworkClasses[image]} aria-hidden={alt ? undefined : true}>
    {(!src || loaded !== src) && <span className="poster-placeholder">
      {!tag ? `无${artworkNames[image]}` : unavailable ? "图片加载失败" : "正在加载图片…"}
    </span>}
    {src && failed !== src && <img src={src} alt={alt ?? ""} loading={eager || isDesktop ? "eager" : "lazy"} decoding="async"
      className={loaded === src ? undefined : "opacity-0"} onLoad={() => setLoaded(src)} onError={() => setFailed(src)} />}
  </span>;
}
