package com.codingtools.mcp.mobile;

import android.app.Activity;
import android.annotation.SuppressLint;
import android.widget.Toast;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.graphics.Color;
import android.graphics.Insets;
import android.net.Uri;
import android.net.http.SslError;
import android.os.Build;
import android.os.Bundle;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.webkit.CookieManager;
import android.webkit.SslErrorHandler;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebStorage;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class MainActivity extends Activity {
    private final ExecutorService network = Executors.newSingleThreadExecutor();
    private FrameLayout root;
    private ScrollView settings;
    private EditText address, password;
    private CheckBox remember, allowHttp;
    private TextView status;
    private Button connect;
    private WebView web;
    private ProfileStore store;
    private ValueCallback<Uri[]> fileCallback;
    private int generation;
    private boolean busy;
    private long lastRenewal;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        store = new ProfileStore(this);
        root = new FrameLayout(this); root.setBackgroundColor(Color.rgb(17, 21, 29));
        setContentView(root);
        configureInsets();
        WebView.setWebContentsDebuggingEnabled(false);
        buildSettings();
        if (Build.VERSION.SDK_INT >= 33)
            getOnBackInvokedDispatcher().registerOnBackInvokedCallback(0, this::back);
        try {
            ConnectionProfile saved = store.load();
            if (saved != null) {
                address.setText(saved.url); password.setText(saved.password);
                remember.setChecked(!saved.password.isEmpty()); allowHttp.setChecked(saved.allowHttp);
                if (!saved.password.isEmpty()) connect(saved, true);
            }
        } catch (Exception ignored) {
            store.clear(); status.setText("已保存连接无法解密，请重新输入。");
        }
    }

    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private void configureInsets() {
        getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        if (Build.VERSION.SDK_INT >= 30) getWindow().setDecorFitsSystemWindows(false);
        else getWindow().getDecorView().setSystemUiVisibility(View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION);
        root.setOnApplyWindowInsetsListener((view, insets) -> {
            if (Build.VERSION.SDK_INT >= 30) {
                Insets bars = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
                Insets keyboard = insets.getInsets(WindowInsets.Type.ime());
                view.setPadding(bars.left, bars.top, bars.right, Math.max(bars.bottom, keyboard.bottom));
            } else {
                view.setPadding(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                        insets.getSystemWindowInsetRight(), insets.getSystemWindowInsetBottom());
            }
            return insets.consumeSystemWindowInsets();
        });
        root.requestApplyInsets();
    }
    private TextView text(String value, int size) {
        TextView view = new TextView(this); view.setText(value); view.setTextSize(size);
        view.setTextColor(Color.rgb(229, 235, 246)); view.setPadding(0, dp(10), 0, dp(10));
        return view;
    }
    private void buildSettings() {
        settings = new ScrollView(this); settings.setFillViewport(true);
        LinearLayout form = new LinearLayout(this); form.setOrientation(LinearLayout.VERTICAL);
        form.setGravity(Gravity.CENTER_VERTICAL); form.setPadding(dp(24), dp(24), dp(24), dp(24));
        settings.addView(form, new ScrollView.LayoutParams(-1, -1)); root.addView(settings, new FrameLayout.LayoutParams(-1, -1));
        form.addView(text("Coding Tools", 32)); form.addView(text("连接你的 MCP 工作台", 18));
        form.addView(text("输入电脑“浏览器分享”中的链接和分享口令。", 14));
        address = new EditText(this); address.setHint("https://你的分享地址"); address.setSingleLine(true);
        address.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        address.setContentDescription("分享链接"); form.addView(address);
        password = new EditText(this); password.setHint("分享口令"); password.setSingleLine(true);
        password.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        password.setContentDescription("分享口令"); password.setSaveEnabled(false);
        password.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        form.addView(password);
        remember = new CheckBox(this); remember.setText("在此设备加密记住口令"); remember.setChecked(true); form.addView(remember);
        allowHttp = new CheckBox(this); allowHttp.setText(R.string.allow_http); form.addView(allowHttp);
        connect = new Button(this); connect.setText("连接工作台"); form.addView(connect);
        connect.setOnClickListener(view -> {
            if (busy) return;
            try { connect(ConnectionProfile.create(address.getText().toString(), password.getText().toString(), allowHttp.isChecked()), remember.isChecked()); }
            catch (IllegalArgumentException e) { status.setText(e.getMessage()); }
        });
        Button clear = new Button(this); clear.setText("清除已保存连接"); form.addView(clear);
        clear.setOnClickListener(view -> {
            showSettings(); boolean cleared = store.clear(); password.setText(""); address.setText("");
            CookieManager.getInstance().removeAllCookies(null); WebStorage.getInstance().deleteAllData();
            status.setText(cleared ? "已清除保存的链接、口令和网页数据。" : "无法清除保存的连接，请重试或在系统设置清除应用数据。");
        });
        status = text("连接后网页占满可用屏幕；返回键可回到此页。", 13); status.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        form.addView(status);
    }
    private void connect(ConnectionProfile profile, boolean savePassword) {
        int request = ++generation; busy = true; connect.setEnabled(false); status.setText("正在安全连接…");
        settings.setVisibility(View.VISIBLE);
        network.execute(() -> {
            try {
                String token = NativeLogin.login(profile);
                runOnUiThread(() -> {
                    if (isDestroyed() || request != generation) return;
                    busy = false; connect.setEnabled(true);
                    try { store.save(profile, savePassword); }
                    catch (Exception ignored) { Toast.makeText(this, "已连接，但无法保存口令；下次需重新输入。", Toast.LENGTH_LONG).show(); }
                    openPage(profile, token, request);
                });
            } catch (Exception e) {
                runOnUiThread(() -> {
                    if (isDestroyed() || request != generation) return;
                    busy = false; connect.setEnabled(true);
                    status.setText(e instanceof java.io.IOException ? e.getMessage() : "连接失败，请检查分享链接和口令。");
                });
            }
        });
    }
    // The selected workbench is a JavaScript app; no JavaScript/native bridge is exposed.
    @SuppressLint("SetJavaScriptEnabled")
    private void openPage(ConnectionProfile profile, String token, int request) {
        destroyWeb();
        WebView page = new WebView(this); web = page; page.setVisibility(View.INVISIBLE);
        WebSettings options = page.getSettings();
        options.setJavaScriptEnabled(true); options.setDomStorageEnabled(true);
        options.setAllowFileAccess(false); options.setAllowContentAccess(true);
        options.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        options.setSupportZoom(true); options.setBuiltInZoomControls(true); options.setDisplayZoomControls(false);
        options.setUserAgentString(options.getUserAgentString() + " CodingToolsMobile/0.1.0");
        CookieManager.getInstance().setAcceptThirdPartyCookies(page, false);
        page.setWebViewClient(new WebViewClient() {
            private String pendingToken = token;
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest navigation) {
                String url = navigation.getUrl().toString();
                if (profile.sameOrigin(url)) return false;
                String scheme = navigation.getUrl().getScheme();
                if (navigation.isForMainFrame() && navigation.hasGesture() && ("https".equals(scheme) || "http".equals(scheme))) {
                    try { startActivity(new Intent(Intent.ACTION_VIEW, navigation.getUrl())); }
                    catch (ActivityNotFoundException ignored) { status.setText("未找到可打开此链接的浏览器。"); }
                }
                return true;
            }
            @Override public void onPageFinished(WebView view, String url) {
                if (request != generation || !profile.sameOrigin(url) || !profile.sameOrigin(view.getUrl())) return;
                if (pendingToken != null) {
                    String installing = pendingToken; pendingToken = null;
                    view.evaluateJavascript(NativeLogin.sessionScript(profile, installing), result -> {
                        if (request != generation || !profile.sameOrigin(view.getUrl())) return;
                        if ("true".equals(result)) view.reload();
                        else { showSettings(); status.setText("网页会话初始化失败，请重试。"); }
                    });
                } else {
                    status.setText(""); settings.setVisibility(View.GONE); page.setVisibility(View.VISIBLE);
                    ((android.view.inputmethod.InputMethodManager)getSystemService(INPUT_METHOD_SERVICE)).hideSoftInputFromWindow(password.getWindowToken(), 0);
                }
            }
            @Override public void onReceivedSslError(WebView view, SslErrorHandler handler, SslError error) {
                handler.cancel(); if (request == generation) { showSettings(); status.setText("证书验证失败，请检查分享地址和服务器证书。"); }
            }
            @Override public void onReceivedError(WebView view, WebResourceRequest resource, WebResourceError error) {
                if (resource.isForMainFrame() && request == generation) { showSettings(); status.setText("网页未能加载，请检查网络后重新连接。"); }
            }
            @Override public void onReceivedHttpError(WebView view, WebResourceRequest resource, WebResourceResponse response) {
                if (request != generation || !profile.sameOrigin(resource.getUrl().toString())) return;
                if (response.getStatusCode() == 401 && "/browser/invoke".equals(resource.getUrl().getPath())
                        && !busy && android.os.SystemClock.elapsedRealtime() - lastRenewal > 30000) {
                    lastRenewal = android.os.SystemClock.elapsedRealtime();
                    connect(profile, remember.isChecked());
                } else if (resource.isForMainFrame() && response.getStatusCode() >= 400) {
                    showSettings(); status.setText(getString(R.string.http_error, response.getStatusCode()));
                }
            }
        });
        page.setWebChromeClient(new WebChromeClient() {
            @Override public boolean onShowFileChooser(WebView view, ValueCallback<Uri[]> callback, FileChooserParams params) {
                if (fileCallback != null) fileCallback.onReceiveValue(null);
                fileCallback = callback;
                Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE);
                intent.setType("*/*"); intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, params.getMode() == FileChooserParams.MODE_OPEN_MULTIPLE);
                String[] types = params.getAcceptTypes(); if (types.length > 0 && !types[0].isEmpty()) intent.putExtra(Intent.EXTRA_MIME_TYPES, types);
                try { startActivityForResult(intent, 41); }
                catch (ActivityNotFoundException ignored) { fileCallback = null; callback.onReceiveValue(null); }
                return true;
            }
        });
        root.addView(page, 0, new FrameLayout.LayoutParams(-1, -1)); page.loadUrl(profile.url);
    }
    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request == 41 && fileCallback != null) {
            Uri[] files = null;
            if (result == RESULT_OK && data != null) {
                if (data.getClipData() != null) {
                    files = new Uri[data.getClipData().getItemCount()];
                    for (int i = 0; i < files.length; i++) files[i] = data.getClipData().getItemAt(i).getUri();
                } else if (data.getData() != null) files = new Uri[]{data.getData()};
            }
            fileCallback.onReceiveValue(files); fileCallback = null;
        }
    }
    private void showSettings() {
        generation++; busy = false; connect.setEnabled(true); destroyWeb(); settings.setVisibility(View.VISIBLE);
    }
    private void destroyWeb() {
        if (fileCallback != null) { fileCallback.onReceiveValue(null); fileCallback = null; }
        if (web != null) { root.removeView(web); web.stopLoading(); web.destroy(); web = null; }
    }
    private void back() {
        if (web != null && web.getVisibility() == View.VISIBLE) {
            if (web.canGoBack()) web.goBack(); else showSettings();
        } else if (busy || web != null) showSettings(); else finish();
    }
    @SuppressWarnings("deprecation") @Override public void onBackPressed() { back(); }
    @Override protected void onDestroy() { generation++; destroyWeb(); network.shutdownNow(); super.onDestroy(); }
}
