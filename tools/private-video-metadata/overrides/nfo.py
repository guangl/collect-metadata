"""NFO document generation."""

import re
from xml.dom import minidom
from xml.etree import ElementTree as ET

from .localize import localize
from .publisher import official_url
from .sources import JAVBUS, _same_code


def _add_supplemental_fields(root, data):
    code = root.findtext("num")
    official = official_url(code) if code else None
    website = root.findtext("website") or data.get("website")
    sources = (root.findtext("source") or "").split("+")
    collections = root.findall("set/name")
    series = collections[0].text if len(collections) == 1 else data.get("series")
    fields = {
        "country": "Japan" if official and website == official else None,
        "publisher": root.findtext("label") or data.get("label"),
        "series": series,
        "javbusid": data.get("javbusid") or (f"{JAVBUS}/{code}" if code and "javbus" in sources else None),
        "javdbid": data.get("javdbid"),
    }
    added = []
    for name, value in fields.items():
        if value and root.find(name) is None:
            ET.SubElement(root, name).text = value.strip()
            added.append(name)
    return added


def supplement_nfo(content, code, data):
    """Append sourced fields to an existing NFO for the matching catalog number."""
    parser = ET.XMLParser(target=ET.TreeBuilder(insert_comments=True, insert_pis=True))
    root = ET.fromstring(content, parser=parser)
    if root.tag != "movie" or not _same_code(root.findtext("num"), code):
        raise ValueError("NFO does not match the requested catalog number")
    added = _add_supplemental_fields(root, data)
    if not added:
        return content, []
    ET.indent(root, space="  ")
    return ET.tostring(root, encoding="utf-8", xml_declaration=True) + b"\n", added


def build_nfo(data, tags, folder_name, video_info=None):
    root = ET.Element("movie")

    def add(parent, tag, text):
        if text is None or text == "":
            return None
        element = ET.SubElement(parent, tag)
        element.text = str(text)
        return element

    code = data.get("dvdid") or data.get("code")

    def strip_code(text):
        text = (text or "").strip()
        return text[len(code) :].strip() if text.upper().startswith(code.upper()) else text

    original_title = strip_code(data.get("title"))
    display_title = strip_code(data.get("title_cn")) or original_title

    add(root, "title", f"{code} {display_title}".strip())
    add(root, "originaltitle", original_title or display_title)
    add(root, "sorttitle", code)
    add(root, "plot", data.get("plot"))
    add(root, "originalplot", data.get("plot"))
    add(root, "website", data.get("website"))
    add(root, "num", code)

    unique_id = ET.SubElement(root, "uniqueid")
    unique_id.set("type", "num")
    unique_id.set("default", "true")
    unique_id.text = code

    digital_release = data.get("digital_release")
    disc_release = data.get("disc_release")
    catalog_release = data.get("release")
    premiere = digital_release or catalog_release or disc_release
    add(root, "premiered", premiere)
    add(root, "releasedate", disc_release)
    if premiere and re.match(r"^\d{4}", premiere):
        add(root, "year", premiere[:4])
    dates = []
    if digital_release:
        dates.append(f"线上配信 {digital_release}")
    elif catalog_release and catalog_release != disc_release:
        dates.append(f"目录日期 {catalog_release}")
    if disc_release:
        dates.append(f"实体发行 {disc_release}")
    add(root, "tagline", " · ".join(dates))

    video_info = video_info or {}
    add(root, "runtime", video_info.get("minutes") or data.get("runtime"))
    if video_info:
        details = ET.SubElement(ET.SubElement(root, "fileinfo"), "streamdetails")
        video = ET.SubElement(details, "video")
        for key, name in (
            ("codec_name", "codec"), ("width", "width"), ("height", "height"),
            ("duration_seconds", "durationinseconds"),
        ):
            add(video, name, video_info.get(key))
    add(root, "studio", data.get("studio"))
    add(root, "maker", data.get("studio"))
    add(root, "label", data.get("label"))
    add(root, "director", data.get("director"))

    if data.get("series"):
        collection = ET.SubElement(root, "set")
        add(collection, "name", data["series"])

    genres = list(dict.fromkeys((*data.get("genres", []), *tags)))
    for genre in genres:
        add(root, "genre", genre)
    actor_names = [actor.get("name") for actor in data.get("actors", [])]
    prefix, separator, _ = code.partition("-")
    extra_tags = [prefix if separator and prefix.isalpha() else None, data.get("series"), *actor_names]
    for tag in dict.fromkeys([*genres, *extra_tags]):
        add(root, "tag", tag)

    for actor in data.get("actors", []):
        actor_element = ET.SubElement(root, "actor")
        add(actor_element, "name", actor.get("name"))
        add(actor_element, "type", "Actor")
        if actor.get("thumb"):
            add(actor_element, "thumb", actor["thumb"])

    add(root, "poster", "poster.jpg")
    fanart = ET.SubElement(root, "fanart")
    add(fanart, "thumb", "fanart.jpg")

    add(root, "source", data.get("source"))
    add(root, "originalfolder", folder_name)

    localize(root)
    _add_supplemental_fields(root, data)
    raw = ET.tostring(root, encoding="utf-8")
    return minidom.parseString(raw).toprettyxml(indent="  ", encoding="UTF-8")
