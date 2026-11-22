# Makes the signed test bundles extension_sign.rs's tests read, the way
# `mcpb sign` does (src/node/sign.ts in modelcontextprotocol/mcpb): a
# detached PKCS#7 over the zip, the zip's EOCD comment length grown by the
# block, then MCPB_SIG_V1 <u32 LE length> <DER> MCPB_SIG_END appended.
# The keys are made in a temporary folder and thrown away.
#
#   python -I src-tauri/src/testdata/make_signed_bundles.py   (needs openssl on PATH)
import io, os, struct, subprocess, tempfile, zipfile

HERE = os.path.dirname(os.path.abspath(__file__))


def run(*args, cwd):
    subprocess.run(args, cwd=cwd, check=True, capture_output=True)


def bundle():
    buf = io.BytesIO()
    # stored, so a test can find and change a byte of the content
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_STORED) as z:
        z.writestr("manifest.json", '{"manifest_version":"0.3","name":"signed-demo","version":"1.0.0","server":{"type":"node","entry_point":"index.js"}}')
        z.writestr("index.js", "console.log('signed')\n")
    return buf.getvalue()


def signed(zip_bytes, sig):
    block = b"MCPB_SIG_V1" + struct.pack("<I", len(sig)) + sig + b"MCPB_SIG_END"
    out = bytearray(zip_bytes)
    eocd = out.rfind(b"PK\x05\x06")
    (comment,) = struct.unpack_from("<H", out, eocd + 20)
    struct.pack_into("<H", out, eocd + 20, comment + len(block))
    return bytes(out) + block


def main():
    with tempfile.TemporaryDirectory() as d:
        z = bundle()
        open(os.path.join(d, "b.zip"), "wb").write(z)
        open(os.path.join(d, "leaf.ext"), "w").write("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=codeSigning\n")
        run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key", "-out", "ca.pem", "-days", "36500",
            "-subj", "/CN=ClaudeRust Test Root", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign", cwd=d)
        run("openssl", "req", "-newkey", "rsa:2048", "-nodes", "-keyout", "leaf.key", "-out", "leaf.csr", "-subj", "/CN=Example Publisher/O=Example Inc", cwd=d)
        run("openssl", "x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-out", "leaf.pem", "-days", "36500", "-extfile", "leaf.ext", cwd=d)
        run("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "self.key", "-out", "self.pem", "-days", "36500",
            "-subj", "/CN=Self Publisher", "-addext", "extendedKeyUsage=codeSigning", cwd=d)
        run("openssl", "x509", "-in", "ca.pem", "-outform", "DER", "-out", "ca.der", cwd=d)
        for who, out in (("leaf", "signed_ca.mcpb"), ("self", "signed_self.mcpb")):
            run("openssl", "cms", "-sign", "-binary", "-md", "sha256", "-in", "b.zip", "-signer", f"{who}.pem", "-inkey", f"{who}.key",
                "-outform", "DER", "-out", f"{who}.sig", cwd=d)
            open(os.path.join(HERE, out), "wb").write(signed(z, open(os.path.join(d, f"{who}.sig"), "rb").read()))
        open(os.path.join(HERE, "test_root.der"), "wb").write(open(os.path.join(d, "ca.der"), "rb").read())


main()
