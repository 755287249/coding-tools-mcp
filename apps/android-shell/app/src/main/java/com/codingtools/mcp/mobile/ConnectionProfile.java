package com.codingtools.mcp.mobile;

import java.net.URI;
import java.util.Locale;

/** Validates URLs before either native authentication or WebView navigation. */
final class ConnectionProfile {
    final String url, origin, password;
    final boolean allowHttp;

    private ConnectionProfile(String url, String origin, String password, boolean allowHttp) {
        this.url = url; this.origin = origin; this.password = password; this.allowHttp = allowHttp;
    }

    static ConnectionProfile create(String input, String password, boolean allowHttp) {
        String value = input.trim();
        if (value.isEmpty() || value.length() > 4096 || value.chars().anyMatch(c -> c <= 32 || c == 127 || c == '\\'))
            throw new IllegalArgumentException("请输入完整的 HTTP(S) 分享链接");
        try {
            URI uri = new URI(value);
            String scheme = uri.getScheme() == null ? "" : uri.getScheme().toLowerCase(Locale.ROOT);
            if ((!scheme.equals("https") && !scheme.equals("http")) || uri.getHost() == null
                    || uri.getRawUserInfo() != null || uri.getPort() == 0 || uri.getPort() > 65535)
                throw new IllegalArgumentException("链接需要有效域名或 IP，不能包含账号或密码");
            if (scheme.equals("http") && !allowHttp)
                throw new IllegalArgumentException("HTTP 不加密；局域网连接请先勾选允许 HTTP");
            int port = uri.getPort();
            if ((scheme.equals("https") && port == 443) || (scheme.equals("http") && port == 80)) port = -1;
            String origin = new URI(scheme, null, uri.getHost().toLowerCase(Locale.ROOT), port, "", null, null).toASCIIString();
            String path = uri.getRawPath();
            String url = origin + (path == null || path.isEmpty() ? "/" : path)
                    + (uri.getRawQuery() == null ? "" : "?" + uri.getRawQuery())
                    + (uri.getRawFragment() == null ? "" : "#" + uri.getRawFragment());
            return new ConnectionProfile(url, origin, password, allowHttp);
        } catch (java.net.URISyntaxException e) {
            throw new IllegalArgumentException("分享链接格式不正确");
        }
    }

    boolean sameOrigin(String candidate) {
        try { return origin.equals(create(candidate, "", allowHttp).origin); }
        catch (RuntimeException e) { return false; }
    }
}
