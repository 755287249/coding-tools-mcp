package com.codingtools.mcp.mobile;

import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.util.Arrays;
import java.util.Base64;
import javax.crypto.Cipher;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

final class SecretBox {
    private static final byte[] AAD = "coding-tools-mobile:profile:v1".getBytes(StandardCharsets.UTF_8);
    static String encrypt(SecretKey key, String text) throws GeneralSecurityException {
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.ENCRYPT_MODE, key);
        cipher.updateAAD(AAD);
        byte[] iv = cipher.getIV();
        if (iv.length != 12) throw new GeneralSecurityException("Unexpected nonce length");
        byte[] encrypted = cipher.doFinal(text.getBytes(StandardCharsets.UTF_8));
        byte[] envelope = new byte[13 + encrypted.length];
        envelope[0] = 1;
        System.arraycopy(iv, 0, envelope, 1, 12);
        System.arraycopy(encrypted, 0, envelope, 13, encrypted.length);
        return Base64.getEncoder().encodeToString(envelope);
    }
    static String decrypt(SecretKey key, String encoded) throws GeneralSecurityException {
        byte[] envelope;
        try { envelope = Base64.getDecoder().decode(encoded); }
        catch (IllegalArgumentException e) { throw new GeneralSecurityException("Invalid stored profile"); }
        if (envelope.length < 29 || envelope[0] != 1) throw new GeneralSecurityException("Invalid stored profile");
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.DECRYPT_MODE, key, new GCMParameterSpec(128, Arrays.copyOfRange(envelope, 1, 13)));
        cipher.updateAAD(AAD);
        return new String(cipher.doFinal(envelope, 13, envelope.length - 13), StandardCharsets.UTF_8);
    }
}
