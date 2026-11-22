# Private video metadata worker overlay

Build this directory against the NAS's existing local `homelab/private-metadata:13d1b6a-reviewed` image. Set the private metadata Compose entrypoint to `["python", "-m", "javlibrarian.worker"]` and use the new image. Remove legacy bind mounts for the worker and publisher Python files. Keep the library, translation model, Emby key, and portrait helper mounts.

The watcher prepares stable single-video imports as `NUMBER/NUMBER.ext` and writes `NUMBER.nfo` alongside the video. Matching subtitles follow the video basename, including their language suffixes. Filename-derived tags are retained as a video attribute for retry, and UHD resolution is measured by ffprobe. Duplicate numbers and multi-video inputs are deferred for manual selection.

The worker saves the original `Samples/sampleN.jpg` and copies each image to the Emby-compatible `extrafanart/fanartN.jpg`. The NFO writer includes verified source links and known country, publisher and series values. The additive NFO helper preserves existing descriptions, cast, and other fields.

For supported publishers, the worker verifies the catalog number on the publisher's product page and uses it for the original title, studio, series, label and physical release date. It keeps retailer streaming dates separate from physical and catalog dates. It preserves numbered series names when translating a title and keeps the original synopsis alongside its localized text. New per-video NFOs use the measured runtime and stream details from `ffprobe` when available. Shared folder NFOs use the catalog runtime when there are multiple videos.

The worker checks JavBus, JavDB and the supported publisher before localizing each field. It uses a matching catalog's Chinese title and synopsis when available and checks R18.dev for a missing Japanese synopsis. The local model translates display text only where no Chinese catalog text is available. Category labels use existing catalog wording and fixed term mappings. Unrecognized labels and text that fails translation checks retain their source wording.

The Python overlays derive from [JavLibrarian](https://github.com/syy674998887/JavLibrarian) at `13d1b6a` and the NAS's existing customizations. The upstream files are available under the [Apache 2.0 license](https://github.com/syy674998887/JavLibrarian/blob/13d1b6a/LICENSE).
