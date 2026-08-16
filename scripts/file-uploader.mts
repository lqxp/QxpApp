// @ts-nocheck
const DEFAULT_BASE = "https://download.wf";

function getArg(name: string): string | undefined {
  const index = Bun.argv.indexOf(`--${name}`);
  return index !== -1 ? Bun.argv[index + 1] : undefined;
}

interface UploadResult {
  url: string;
  key: string;
  protected: boolean;
}

async function uploadToDownloadWf(
  filePath: string,
  opts: { password?: string; base?: string } = {},
): Promise<UploadResult> {
  const file = Bun.file(filePath);
  if (!(await file.exists())) {
    throw new Error(`File not found: ${filePath}`);
  }

  const base = (opts.base ?? DEFAULT_BASE).replace(/\/+$/, "");
  const fileName = filePath.split("/").pop() ?? "file";

  const form = new FormData();
  form.append("file", file, fileName);
  if (opts.password) {
    form.append("password", opts.password);
  }

  const response = await fetch(`${base}/api/upload`, {
    method: "POST",
    body: form,
  });

  let result: any;
  try {
    result = await response.json();
  } catch {
    throw new Error(`download.wf returned a non-JSON response (HTTP ${response.status})`);
  }

  if (!response.ok) {
    const message = typeof result?.error === "string" ? result.error : `HTTP ${response.status}`;
    throw new Error(`download.wf upload failed: ${message}`);
  }

  if (typeof result?.url !== "string" || typeof result?.key !== "string") {
    throw new Error(`download.wf returned an unexpected response: ${JSON.stringify(result)}`);
  }

  // The API returns an absolute url when the server has a public base URL
  // configured, otherwise a relative "/key" — normalize to absolute here.
  const url = result.url.startsWith("http") ? result.url : `${base}${result.url}`;

  return { url, key: result.key, protected: !!result.protected };
}

const filePath = getArg("file");
if (!filePath) {
  console.error("Usage: bun run ./downloadwf-uploader.mts --file <path> [--password <pass>] [--base <url>]");
  process.exit(1);
}

try {
  const { url, key, protected: isProtected } = await uploadToDownloadWf(filePath, {
    password: getArg("password"),
    base: getArg("base"),
  });
  console.log(url);
  console.error(isProtected ? `(password-protected; delete key: ${key})` : `(delete key: ${key})`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
