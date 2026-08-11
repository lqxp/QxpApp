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

    const archive = new Bun.Archive({
        [file.name ?? "file"]: file,
    });

    const archivePath = `${filePath}.tar.gz`;

    await Bun.write(archivePath, archive);

    try {
        const archiveFile = Bun.file(archivePath);

        const form = new FormData();
        form.append("reqtype", "fileupload");
        form.append("fileToUpload", archiveFile);

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
    } finally {
        await Bun.file(archivePath).delete();
    }
}

const filePath = getArg("file");

if (!filePath) {
    console.error("Usage: bun run ./catbox-uploader.mts --file <path>");
    process.exit(1);
}

try {
    console.log(await uploadToCatbox(filePath));
} catch (error) {
    console.error(
        error instanceof Error ? error.message : String(error)
    );

    process.exit(1);
}
