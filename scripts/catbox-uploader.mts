// @ts-nocheck

const CATBOX_API = "https://catbox.moe/user/api.php";

function getArg(name: string): string | undefined {
    const index = Bun.argv.indexOf(`--${name}`);
    return index !== -1 ? Bun.argv[index + 1] : undefined;
}

async function uploadToCatbox(filePath: string): Promise<string> {
    const file = Bun.file(filePath);

    if (!(await file.exists())) {
        throw new Error(`File not found: ${filePath}`);
    }

    const form = new FormData();

    form.append("reqtype", "fileupload");
    form.append("fileToUpload", file);

    const response = await fetch(CATBOX_API, {
        method: "POST",
        body: form,
    });

    const result = (await response.text()).trim();

    if (!response.ok) {
        throw new Error(
            `Catbox returned HTTP ${response.status}: ${result}`
        );
    }

    if (!result.startsWith("https://files.catbox.moe/")) {
        throw new Error(`Catbox upload failed: ${result}`);
    }

    return result;
}

const filePath = getArg("file");

if (!filePath) {
    console.error("Usage: bun run ./file.ts --file <path>");
    process.exit(1);
}

try {
    const url = await uploadToCatbox(filePath);
    console.log(url);
} catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
}
