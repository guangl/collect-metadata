# Private video metadata worker overlay

Build this directory against the NAS's existing local `homelab/private-metadata:13d1b6a-reviewed` image. Point the private metadata Compose worker at the new image and remove the old bind mount for `javlibrarian/publisher.py` so the worker loads the publisher adapter from the image. Keep the other Compose mounts.

The worker saves the original `Samples/sampleN.jpg` and copies each image to the Emby-compatible `extrafanart/fanartN.jpg`. The NFO writer includes verified source links and known country, publisher and series values. The additive NFO helper preserves existing descriptions, cast, and other fields.

The Python overlays derive from [JavLibrarian](https://github.com/syy674998887/JavLibrarian) at `13d1b6a` and the NAS's existing customizations. The upstream files are available under the [Apache 2.0 license](https://github.com/syy674998887/JavLibrarian/blob/13d1b6a/LICENSE).
