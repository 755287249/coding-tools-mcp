package com.codingtools.mcp.mobile;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.util.Base64;
import android.webkit.WebView;
import androidx.webkit.JavaScriptReplyProxy;
import androidx.webkit.WebViewCompat;
import androidx.webkit.WebViewFeature;
import java.io.File;
import java.io.FileOutputStream;
import java.io.FileInputStream;
import java.io.OutputStream;
import java.util.Collections;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.json.JSONObject;

/** Receives bounded file bytes only from the selected workbench's main frame. */
final class DownloadBridge {
    static final int SAVE_REQUEST = 42;
    private final Activity activity;
    private final ExecutorService io = Executors.newSingleThreadExecutor();
    private File file;
    private FileOutputStream output;
    private String name, mime, finishRequest;
    private JavaScriptReplyProxy finishReply;
    private long expected, written;
    private boolean choosing, closed;
    DownloadBridge(Activity activity) { this.activity = activity; }
    boolean attach(WebView page, ConnectionProfile profile) {
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) return false;
        WebViewCompat.addWebMessageListener(page, "CodingToolsDownloads", Collections.singleton(profile.origin),
            (view, message, sourceOrigin, mainFrame, reply) -> {
                if (!mainFrame || !profile.sameOrigin(sourceOrigin.toString()) || !profile.sameOrigin(view.getUrl())) return;
                String raw = message.getData();
                if (raw == null || raw.length() > 256 * 1024) return;
                synchronized(this) { if (!closed) io.execute(() -> receive(raw, reply)); }
            });
        return true;
    }
    private void answer(JavaScriptReplyProxy reply, String id, String error) {
        activity.runOnUiThread(() -> {
            try { JSONObject result = new JSONObject().put("requestId", id).put("ok", error == null);
                if (error != null) result.put("error", error);
                reply.postMessage(result.toString());
            } catch (Exception ignored) { /* WebView may have closed. */ }
        });
    }
    private synchronized void receive(String raw, JavaScriptReplyProxy reply) {
        String id = "";
        try {
            JSONObject data = new JSONObject(raw); id = data.getString("requestId");
            if (closed) throw new IllegalStateException("Download view was closed");
            switch (data.getString("action")) {
                case "start":
                    if (file != null || choosing) throw new IllegalStateException("Finish the current save first");
                    long size = data.getLong("size");
                    if (size < 0 || size > 32L * 1024 * 1024) throw new IllegalArgumentException("File exceeds the mobile save limit");
                    name = data.optString("name", "download").replaceAll("[\\\\/\\p{Cntrl}]", "_");
                    if (name.isEmpty()) name = "download";
                    if (name.length() > 200) name = name.substring(0, 200);
                    mime = data.optString("mime", "application/octet-stream").split(";", 2)[0];
                    if (!mime.matches("[A-Za-z0-9.+-]+/[A-Za-z0-9.+-]+")) mime = "application/octet-stream";
                    file = File.createTempFile("ctmcp-download-", ".tmp", activity.getCacheDir());
                    output = new FileOutputStream(file); expected = size; written = 0;
                    break;
                case "chunk":
                    if (output == null || choosing || data.getLong("offset") != written) throw new IllegalArgumentException("Invalid download offset");
                    byte[] bytes = Base64.decode(data.getString("data"), Base64.NO_WRAP);
                    if (bytes.length == 0 || bytes.length > 128 * 1024 || written + bytes.length > expected) throw new IllegalArgumentException("Invalid download chunk");
                    output.write(bytes); written += bytes.length;
                    break;
                case "finish":
                    if (output == null || written != expected || choosing) throw new IllegalArgumentException("Incomplete download");
                    output.close(); output = null; choosing = true; finishReply = reply; finishRequest = id;
                    activity.runOnUiThread(() -> {
                        synchronized(this) { if (closed || !choosing) return; }
                        try { activity.startActivityForResult(new Intent(Intent.ACTION_CREATE_DOCUMENT)
                            .addCategory(Intent.CATEGORY_OPENABLE).setType(mime).putExtra(Intent.EXTRA_TITLE, name), SAVE_REQUEST); }
                        catch (RuntimeException error) { failSave("No system file picker is available"); }
                    });
                    return;
                case "cancel":
                    cleanup(); break;
                default: throw new IllegalArgumentException("Unknown download action");
            }
            answer(reply, id, null);
        } catch (Exception error) { answer(reply, id, error.getMessage()); }
    }
    synchronized void result(int result, Intent data) {
        if (closed) return;
        io.execute(() -> {
            synchronized (this) {
                if (!choosing || closed) return;
                String error = result == Activity.RESULT_OK ? null : "File save cancelled";
                if (result == Activity.RESULT_OK && data != null && data.getData() != null) {
                    try (FileInputStream input = new FileInputStream(file); OutputStream dest = activity.getContentResolver().openOutputStream(data.getData(), "w")) {
                        if (dest == null) throw new IllegalStateException("Cannot open the selected file");
                        byte[] buffer = new byte[65536]; int count;
                        while ((count = input.read(buffer)) != -1) dest.write(buffer, 0, count);
                    } catch (Exception failure) { error = "Unable to save file: " + failure.getMessage(); }
                }
                answer(finishReply, finishRequest, error); cleanup();
            }
        });
    }
    private synchronized void failSave(String error) { if (finishReply != null) answer(finishReply, finishRequest, error); cleanup(); }
    private void cleanup() {
        try { if (output != null) output.close(); } catch (Exception ignored) {}
        output = null; if (file != null) file.delete(); file = null; choosing = false; finishReply = null; finishRequest = null;
    }
    synchronized void close() { closed = true; cleanup(); io.shutdown(); }
}
