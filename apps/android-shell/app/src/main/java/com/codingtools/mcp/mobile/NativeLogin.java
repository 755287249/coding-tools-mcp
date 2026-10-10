package com.codingtools.mcp.mobile;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import org.json.JSONObject;

final class NativeLogin {
    static String login(ConnectionProfile profile) throws Exception {
        if (profile.password.isEmpty()) throw new IOException("请输入分享口令");
        HttpURLConnection connection = (HttpURLConnection) URI.create(profile.origin + "/browser/login").toURL().openConnection();
        try {
            // Never forward a password to a redirected host or downgrade HTTPS.
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout(15000); connection.setReadTimeout(15000);
            connection.setRequestMethod("POST"); connection.setDoOutput(true);
            connection.setRequestProperty("Content-Type", "application/json; charset=utf-8");
            connection.setRequestProperty("Origin", profile.origin);
            connection.setRequestProperty("User-Agent", "Coding-Tools-Mobile/0.1.0");
            byte[] body = new JSONObject().put("password", profile.password).toString().getBytes(StandardCharsets.UTF_8);
            connection.setFixedLengthStreamingMode(body.length);
            try (var output = connection.getOutputStream()) { output.write(body); }
            int status = connection.getResponseCode();
            if (status != 200) {
                if (status >= 300 && status < 400) throw new IOException("链接发生重定向，请填写最终的分享链接");
                if (status == 401 || status == 403) throw new IOException("分享口令错误，或主机未启用此地址的分享");
                if (status == 429) throw new IOException("登录请求过多，请稍后再试");
                throw new IOException("连接失败（HTTP " + status + "），请检查主机的手机分享服务");
            }
            ByteArrayOutputStream output = new ByteArrayOutputStream();
            try (InputStream input = connection.getInputStream()) {
                byte[] buffer = new byte[4096]; int count;
                while ((count = input.read(buffer)) != -1) {
                    if (output.size() + count > 65536) throw new IOException("登录响应过大");
                    output.write(buffer, 0, count);
                }
            }
            String token = new JSONObject(output.toString(StandardCharsets.UTF_8.name())).optString("token", "");
            if (!token.matches("[A-Za-z0-9_-]{16,4096}")) throw new IOException("该地址未返回有效的手机端会话");
            return token;
        } finally { connection.disconnect(); }
    }
    static String sessionScript(ConnectionProfile profile, String token) {
        return "(()=>{if(location.origin!==" + JSONObject.quote(profile.origin) + ")return false;"
                + "sessionStorage.setItem('ctmcp-browser-session'," + JSONObject.quote(token) + ");return true;})()";
    }
}
