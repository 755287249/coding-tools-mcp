package com.codingtools.mcp.mobile;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import java.security.KeyStore;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import org.json.JSONObject;

final class ProfileStore {
    private static final String ALIAS = "ctmcp.mobile.profile.v1";
    private final SharedPreferences prefs;
    ProfileStore(Context context) { prefs = context.getSharedPreferences("connection", Context.MODE_PRIVATE); }
    private SecretKey key(boolean create) throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore"); store.load(null);
        if (store.containsAlias(ALIAS)) return (SecretKey) store.getKey(ALIAS, null);
        if (!create) throw new IllegalStateException("Saved key unavailable");
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256).build());
        return generator.generateKey();
    }
    ConnectionProfile load() throws Exception {
        String saved = prefs.getString("sealed", null);
        if (saved == null) return null;
        JSONObject json = new JSONObject(SecretBox.decrypt(key(false), saved));
        return ConnectionProfile.create(json.getString("url"), json.getString("password"), json.getBoolean("allowHttp"));
    }
    synchronized String session(ConnectionProfile profile) throws Exception {
        String saved=prefs.getString("sealed",null);if(saved==null)return "";
        JSONObject json=new JSONObject(SecretBox.decrypt(key(false),saved));
        ConnectionProfile previous=ConnectionProfile.create(json.getString("url"),json.getString("password"),json.getBoolean("allowHttp"));
        return previous.origin.equals(profile.origin) && previous.password.equals(profile.password) ? json.optString("session", "") : "";
    }
    synchronized void save(ConnectionProfile profile, boolean remember, String token) throws Exception {
        JSONObject json = new JSONObject().put("url", profile.url).put("password", remember ? profile.password : "")
                .put("allowHttp", profile.allowHttp).put("session", remember ? token : "");
        if (!prefs.edit().putString("sealed", SecretBox.encrypt(key(true), json.toString())).commit())
            throw new IllegalStateException("Cannot save connection");
    }
    synchronized void forgetCredentials(ConnectionProfile profile) throws Exception { save(profile, false, ""); }
    synchronized boolean clear() { return prefs.edit().clear().commit(); }
}
