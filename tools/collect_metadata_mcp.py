# /// script
# requires-python = ">=3.10"
# dependencies = ["mcp>=1.27,<2", "httpx>=0.28,<1"]
# ///
"""Expose Collect Metadata's Web API to local MCP clients over stdio."""

import os
import unicodedata
from typing import Annotated, Any, Literal

import httpx
from mcp.server.fastmcp import FastMCP
from mcp.types import ToolAnnotations
from pydantic import BaseModel, Field

server = FastMCP("Collect Metadata")
read_only = ToolAnnotations(readOnlyHint=True)
queue_write = ToolAnnotations(readOnlyHint=False, destructiveHint=False)

MediaKind = Literal["movie", "series", "av"]
Provider = Literal["tmdb", "wikidata", "tvmaze"]
PositiveId = Annotated[int, Field(gt=0)]
Offset = Annotated[int, Field(ge=0)]
Limit = Annotated[int, Field(ge=1, le=100)]
Query = Annotated[str, Field(min_length=1, max_length=160)]


class WorkQuery(BaseModel):
    kind: MediaKind
    query: Query


class EmbyItem(BaseModel):
    id: Annotated[str, Field(pattern=r"^[0-9]{1,20}$")]
    scope: Literal["movie", "series", "episode"]


async def request(method: str, path: str, payload=None):
    base = httpx.URL(os.environ.get("COLLECT_METADATA_WEB_URL", "http://127.0.0.1:8080"))
    if base.scheme not in ("http", "https") or not base.host or base.userinfo or base.query or base.fragment:
        raise ValueError("COLLECT_METADATA_WEB_URL must be an HTTP(S) base URL.")
    token = os.environ.get("COLLECT_METADATA_WEB_TOKEN")
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    if token and base.scheme != "https" and base.host not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("Use an HTTPS base URL when sending a token to a remote host.")
    try:
        async with httpx.AsyncClient(timeout=90, follow_redirects=False, trust_env=False) as client:
            response = await client.request(method, str(base).rstrip("/") + path, headers=headers, json=payload)
    except httpx.RequestError:
        raise ValueError("Could not reach the Collect Metadata Web service.") from None
    if response.status_code == 204:
        return None
    try:
        data = response.json()
    except ValueError:
        raise ValueError(f"Collect Metadata returned HTTP {response.status_code} without JSON.") from None
    if not response.is_success:
        message = data.get("error") if isinstance(data, dict) else None
        raise ValueError(message or f"Collect Metadata returned HTTP {response.status_code}.")
    return data


@server.tool(annotations=read_only)
async def search_media(kind: Literal["movie", "series"], query: Query) -> dict[str, Any]:
    """Find source candidates by title or an explicit source ID such as tmdb:496243."""
    return await request("POST", "/api/search", {"kind": kind, "query": query})


@server.tool(annotations=read_only)
async def list_emby_libraries() -> dict[str, Any]:
    """List the configured Emby libraries and their IDs."""
    return {"libraries": await request("GET", "/api/emby/libraries")}


@server.tool(annotations=read_only)
async def find_emby_works(
    library_id: str, query: str = "", missing: Literal["any", "poster", "overview", "none"] = "none",
    offset: Offset = 0, limit: Limit = 50,
) -> dict[str, Any]:
    """Find works in an Emby library by title, year or missing metadata. Use returned IDs when scraping selected works."""
    works = await request("POST", "/api/emby/preview", library_id)
    term = unicodedata.normalize("NFKC", query.strip()).casefold()
    matches = []
    for work in works:
        values = (work.get("title"), work.get("original_title"), work.get("year"))
        if term and not any(term in unicodedata.normalize("NFKC", str(value)).casefold() for value in values if value):
            continue
        lacks_poster = not work.get("poster")
        lacks_overview = not (work.get("overview") or "").strip()
        if missing == "poster" and not lacks_poster or missing == "overview" and not lacks_overview:
            continue
        if missing == "any" and not (lacks_poster or lacks_overview):
            continue
        matches.append({
            "id": work["id"], "scope": work["kind"], "title": work["title"], "year": work.get("year"),
            "missing_poster": lacks_poster, "missing_overview": lacks_overview,
        })
    return {"total": len(matches), "works": matches[offset:offset + limit]}


@server.tool(annotations=queue_write)
async def start_scrape(
    items: Annotated[list[WorkQuery], Field(min_length=1, max_length=100)], use_ai: bool = False,
) -> dict[str, Any]:
    """Queue media lookups in PostgreSQL. AI mode matches films and series using the server's configured model."""
    payload = [item.model_dump() for item in items]
    path = "/api/batches/ai" if use_ai else "/api/batches"
    return {"batch_id": await request("POST", path, {"items": payload} if use_ai else payload)}


@server.tool(annotations=queue_write)
async def start_emby_scrape(
    items: Annotated[list[EmbyItem], Field(min_length=1, max_length=500)], use_ai: bool = False,
) -> dict[str, Any]:
    """Queue metadata collection for selected Emby items. Use the workspace to preview and confirm media writeback."""
    payload = {"items": [item.model_dump() for item in items], "use_ai": use_ai}
    return {"batch_id": await request("POST", "/api/emby/import-selection", payload)}


@server.tool(annotations=read_only)
async def list_batches(before: PositiveId | None = None) -> dict[str, Any]:
    """List recent batches from the shared workspace, newest first. Use before to fetch older batches."""
    suffix = f"?before={before}" if before is not None else ""
    return {"batches": await request("GET", f"/api/batches{suffix}")}


@server.tool(annotations=read_only)
async def batch_status(batch_id: PositiveId, offset: Offset = 0, limit: Limit = 50) -> dict[str, Any]:
    """Inspect a batch's status, progress, and source candidates. Use the returned row ID to read or correct a result."""
    batch = await request("GET", f"/api/batches/{batch_id}/state")
    rows = batch["rows"]
    return {"status": batch["status"], "total": len(rows), "rows": rows[offset:offset + limit]}


@server.tool(annotations=read_only)
async def batch_result(batch_id: PositiveId, row_id: PositiveId) -> dict[str, Any]:
    """Read normalized scraped metadata and field provenance for one batch row."""
    result = await request("GET", f"/api/batches/{batch_id}/rows/{row_id}/result")
    record = result["record"]
    fields = (
        "media_type", "work_id", "query", "scope", "fields", "field_sources",
        "metadata_languages", "localized_metadata", "match_method",
    )
    return {key: record[key] for key in fields if key in record}


@server.tool(annotations=queue_write)
async def correct_match(
    batch_id: PositiveId, row_id: PositiveId, provider: Provider, source_id: Query,
) -> dict[str, Any]:
    """Queue a new source match on an existing row. A verified result is remembered for later scans."""
    await request("POST", f"/api/batches/{batch_id}/rows/{row_id}/match", {
        "provider": provider, "id": source_id,
    })
    return {"batch_id": batch_id, "row_id": row_id}


if __name__ == "__main__":
    server.run(transport="stdio")
