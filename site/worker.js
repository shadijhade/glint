// Cloudflare's static assets answer a request for a range of bytes with the whole file (200, no Content-Range),
// and Safari does not play a video from a server that does. So the clips come through here first
// (`run_worker_first` in wrangler.toml), and a range gets its bytes (206). Nothing else on the site runs this:
// every other path is served as a static asset, as before.

export default {
  /**
   * @param {Request} request
   * @param {{ ASSETS: { fetch(request: Request): Promise<Response> } }} env
   */
  async fetch(request, env) {
    const asset = await env.ASSETS.fetch(request);
    const wanted = request.headers.get("Range");
    // Only a file as it is stored can be cut: not a 304 or a 404, and not one the assets compressed.
    if (asset.status !== 200 || asset.headers.has("Content-Encoding")) return asset;
    if (request.method !== "GET" || !wanted || moved(request, asset)) return whole(asset);

    const bytes = await asset.arrayBuffer();
    const size = bytes.byteLength;
    const cut = span(wanted, size);
    if (cut === undefined) return whole(new Response(bytes, asset));

    const headers = new Headers(asset.headers);
    headers.set("Accept-Ranges", "bytes");
    headers.delete("Content-Length");
    if (cut === null) {
      headers.set("Content-Range", `bytes */${size}`);
      return new Response(null, { status: 416, headers });
    }
    const [first, last] = cut;
    headers.set("Content-Range", `bytes ${first}-${last}/${size}`);
    return new Response(bytes.slice(first, last + 1), { status: 206, headers });
  },
};

/** The file as it is, saying that a range of it can be asked for. */
function whole(asset) {
  const response = new Response(asset.body, asset);
  response.headers.set("Accept-Ranges", "bytes");
  return response;
}

/** `If-Range` names another version of the file than this one: the rest of it has to be sent again. */
function moved(request, asset) {
  const known = request.headers.get("If-Range");
  return known !== null && known !== asset.headers.get("ETag");
}

/**
 * One range of a file of `size` bytes as `[first, last]`; `null` for a range past its end (416);
 * `undefined` for a header to take no notice of (several ranges, another unit, last before first).
 * @returns {[number, number] | null | undefined}
 */
export function span(header, size) {
  const parts = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
  if (!parts || (parts[1] === "" && parts[2] === "") || size === 0) return undefined;
  if (parts[1] === "") {
    const tail = Number(parts[2]);
    return tail === 0 ? null : [Math.max(size - tail, 0), size - 1];
  }
  const first = Number(parts[1]);
  const last = parts[2] === "" ? size - 1 : Math.min(Number(parts[2]), size - 1);
  if (parts[2] !== "" && Number(parts[2]) < first) return undefined;
  return first >= size ? null : [first, last];
}
