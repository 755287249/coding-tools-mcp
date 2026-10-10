package com.codingtools.mcp.mobile;

import org.junit.Test;
import static org.junit.Assert.*;
import okhttp3.mockwebserver.MockWebServer;
import okhttp3.mockwebserver.MockResponse;
import java.util.concurrent.TimeUnit;
import java.nio.charset.StandardCharsets;
import java.util.Base64;

import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import org.json.JSONObject;

public class ShellContractTest {
    @Test public void originBoundaryPreservesDeepLinkAndNormalizesDefaultPorts() {
        ConnectionProfile p = ConnectionProfile.create("HTTPS://Example.COM:443/workspace/a?chat=x#history", "synthetic", false);
        assertEquals("https://example.com", p.origin);
        assertEquals("https://example.com/workspace/a?chat=x#history", p.url);
        assertTrue(p.sameOrigin("https://example.com:443/elsewhere"));
        for (String url : new String[]{"http://example.com", "https://example.com.evil", "https://example.com:8443", "https://user@example.com", "https://example.com\\@evil.test", "javascript:alert(1)", "file:///etc/passwd"})
            assertFalse(url, p.sameOrigin(url));
    }
    @Test public void httpNeedsExplicitOptInAndInvalidAddressesFail() {
        assertThrows(IllegalArgumentException.class, () -> ConnectionProfile.create("http://192.168.1.9:9000", "x", false));
        assertEquals("http://192.168.1.9:9000", ConnectionProfile.create("http://192.168.1.9:9000", "x", true).origin);
        for (String url : new String[]{"https://example.com:0", "https://example.com:65536", "https://x y", "https://example.com/\nfoo", "//example.com", "https://u:p@example.com", "data:text/html,hi"})
            assertThrows(url, IllegalArgumentException.class, () -> ConnectionProfile.create(url, "x", true));
    }
    @Test public void encryptedProfileUsesFreshNonceAndRejectsTamperingAndWrongKey() throws Exception {
        KeyGenerator generator = KeyGenerator.getInstance("AES"); generator.init(256);
        SecretKey key = generator.generateKey(); String secret = "synthetic password \" 中文";
        String a = SecretBox.encrypt(key, secret), b = SecretBox.encrypt(key, secret);
        assertNotEquals(a, b); assertEquals(secret, SecretBox.decrypt(key, a));
        assertFalse(a.contains(secret)); byte[] altered = Base64.getDecoder().decode(a); altered[altered.length-1] ^= 1;
        assertThrows(Exception.class, () -> SecretBox.decrypt(key, Base64.getEncoder().encodeToString(altered)));
        assertThrows(Exception.class, () -> SecretBox.decrypt(generator.generateKey(), a));
        assertThrows(Exception.class, () -> SecretBox.decrypt(key, "broken"));
    }
    @Test public void loginUsesOriginAndJsonWithoutFollowingPasswordRedirects() throws Exception {
        try (MockWebServer server = new MockWebServer(); MockWebServer other = new MockWebServer()) {
            server.start(); other.start();
            String origin = server.url("/").toString().replaceAll("/$", "");
            String secret = "synthetic \" 密码", token = "0123456789abcdef0123456789abcdef";
            server.enqueue(new MockResponse().setBody("{\"token\":\"" + token + "\"}"));
            server.enqueue(new MockResponse().setResponseCode(307).addHeader("Location", other.url("/other")));
            other.enqueue(new MockResponse().setBody("{\"token\":\"" + token + "\"}"));
            ConnectionProfile profile = ConnectionProfile.create(origin + "/workspace/a", secret, true);
            assertEquals(token, NativeLogin.login(profile));
            var first = server.takeRequest(3, TimeUnit.SECONDS);
            assertNotNull(first); assertEquals("POST", first.getMethod()); assertEquals("/browser/login", first.getPath());
            assertEquals(origin, first.getHeader("Origin"));
            assertEquals(secret, new JSONObject(first.getBody().readUtf8()).getString("password"));
            assertThrows(java.io.IOException.class, () -> NativeLogin.login(profile));
            assertEquals(0, other.getRequestCount()); assertEquals(2, server.getRequestCount());
            String script = NativeLogin.sessionScript(profile, token);
            assertTrue(script.contains("location.origin!==")); assertFalse(script.contains(secret)); assertTrue(script.contains("ctmcp-browser-session"));
        }
    }
}
