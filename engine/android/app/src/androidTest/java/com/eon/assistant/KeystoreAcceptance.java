package com.eon.assistant;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Context;
import android.os.Bundle;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyInfo;
import android.security.keystore.KeyProperties;

import java.security.KeyStore;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.Collections;

import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.SecretKeyFactory;

/** Separate, same-signer instrumentation APK; runs against target code, never packaged in EON. */
public final class KeystoreAcceptance extends Instrumentation {
    private static final String ALIAS = AndroidCredentialBroker.ALIAS;
    private static final String PROVIDER = "AndroidKeyStore";
    private static final String PREFS = "eon-keystore-instrumentation";
    private static AndroidCredentialBroker liveUntilProcessDeath;
    private String phase;
    private String step = "startup";

    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        phase = arguments.getString("phase");
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        try {
            check("com.eon.assistant".equals(getTargetContext().getPackageName()));
            if ("issue".equals(phase)) firstProcess();
            else if ("restart".equals(phase)) secondProcess();
            else throw new AssertionError();
            result.putString("keystore", "PASS-" + phase);
            finish(Activity.RESULT_OK, result);
        } catch (Exception | AssertionError | LinkageError failure) {
            // Only a fixed checkpoint; no values, handles, provider messages or stack traces.
            result.putString("keystore", "FAIL-" + step);
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private static void check(boolean ok) {
        if (!ok) throw new AssertionError();
    }

    private static byte[] fixture() {
        byte[] bytes = new byte[32];
        for (int i = 0; i < bytes.length; i++) bytes[i] = (byte) (i + 1);
        return bytes;
    }

    private static KeyStore store() throws Exception {
        KeyStore store = KeyStore.getInstance(PROVIDER);
        store.load(null);
        return store;
    }

    private static void checkSingleKey() throws Exception {
        KeyStore store = store();
        check(Collections.list(store.aliases()).equals(Collections.singletonList(ALIAS)));
        SecretKey key = (SecretKey) store.getKey(ALIAS, null);
        check(key != null && key.getEncoded() == null && key.getFormat() == null);
        KeyInfo info = (KeyInfo) SecretKeyFactory.getInstance("AES", PROVIDER).getKeySpec(key, KeyInfo.class);
        check(info.getOrigin() == KeyProperties.ORIGIN_GENERATED && info.getKeySize() == 256);
        check(info.getPurposes() == (KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT));
        check(Arrays.equals(info.getBlockModes(), new String[] {"GCM"}));
        check(Arrays.equals(info.getEncryptionPaddings(), new String[] {"NoPadding"}));
    }

    private void firstProcess() throws Exception {
        step = "clean-target";
        check(!store().aliases().hasMoreElements());
        AndroidCredentialBroker.Authority authority = new AndroidCredentialBroker.Authority();
        AndroidCredentialBroker.Owner owner = authority.newOwner(), other = authority.newOwner();
        AndroidCredentialBroker.Owner foreign = new AndroidCredentialBroker.Authority().newOwner();
        byte[] expected = MessageDigest.getInstance("SHA-256").digest(fixture());
        try (AndroidCredentialBroker broker = new AndroidCredentialBroker(authority)) {
            step = "provision";
            byte[] secret = fixture();
            String handle = broker.put(owner.manage, secret);
            check(handle != null && handle.startsWith("secret://android/"));
            check(Arrays.equals(secret, new byte[secret.length]));
            checkSingleKey();

            step = "authorization";
            byte[] denied = fixture();
            check(broker.put(owner.use, denied) == null && Arrays.equals(denied, new byte[denied.length]));
            check(broker.put(foreign.manage, fixture()) == null);
            check(!broker.consumeInspectionProof(owner.manage, handle, expected));
            check(!broker.consumeInspectionProof(other.use, handle, expected));
            check(!broker.consumeInspectionProof(foreign.use, handle, expected));
            check(!broker.release(other.manage, handle) && !broker.release(owner.use, handle));
            check(broker.consumeInspectionProof(owner.use, handle, expected));
            check(!broker.consumeInspectionProof(owner.use, handle, expected));

            step = "bounds-and-release";
            check(broker.put(owner.manage, new byte[0]) == null);
            check(broker.put(owner.manage, new byte[32_768]) == null);
            String[] slots = new String[AndroidCredentialBroker.CAPACITY];
            for (int i = 0; i < slots.length; i++) {
                slots[i] = broker.put(owner.manage, fixture());
                check(slots[i] != null);
            }
            check(broker.put(owner.manage, fixture()) == null);
            for (String slot : slots) {
                check(broker.release(owner.manage, slot));
                check(!broker.consumeInspectionProof(owner.use, slot, expected));
            }
            String mismatch = broker.put(owner.manage, fixture());
            check(mismatch != null && !broker.consumeInspectionProof(owner.use, mismatch, new byte[32]));
            check(!broker.consumeInspectionProof(owner.use, mismatch, expected));

            step = "invalidation";
            String missing = broker.put(owner.manage, fixture()), replaced = broker.put(owner.manage, fixture());
            check(missing != null && replaced != null);
            store().deleteEntry(ALIAS);
            check(!broker.consumeInspectionProof(owner.use, missing, expected));
            check(!store().containsAlias(ALIAS)); // No read-time regeneration.
            check(broker.put(owner.manage, fixture()) != null);
            check(!broker.consumeInspectionProof(owner.use, replaced, expected)); // Wrong GCM key/tag.

            step = "reject-weak-key";
            store().deleteEntry(ALIAS);
            KeyGenerator generator = KeyGenerator.getInstance("AES", PROVIDER);
            generator.init(new KeyGenParameterSpec.Builder(ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setKeySize(128).setBlockModes("GCM").setEncryptionPaddings("NoPadding").build());
            generator.generateKey();
            check(broker.put(owner.manage, fixture()) == null);
            store().deleteEntry(ALIAS);
        }

        step = "shortcut-credential";
        try (InspectionCredential proof = InspectionCredential.Companion.issue()) {
            check(proof != null && proof.redeem() && !proof.redeem());
        }
        step = "close";
        AndroidCredentialBroker closed = new AndroidCredentialBroker(authority);
        String retired = closed.put(owner.manage, fixture());
        check(retired != null);
        closed.close();
        check(!closed.consumeInspectionProof(owner.use, retired, expected));
        check(closed.put(owner.manage, fixture()) == null);

        step = "restart-fixture";
        liveUntilProcessDeath = new AndroidCredentialBroker(authority);
        String old = liveUntilProcessDeath.put(owner.manage, fixture());
        check(old != null);
        // Test-only: carry just the *name*, no secret, ciphertext, grant or key, across force-stop.
        check(getTargetContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                .edit().putString("old-handle", old).commit());
        checkSingleKey();
    }

    private void secondProcess() throws Exception {
        step = "restarted";
        check(liveUntilProcessDeath == null);
        checkSingleKey();
        String old = getTargetContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("old-handle", null);
        check(old != null);
        AndroidCredentialBroker.Authority authority = new AndroidCredentialBroker.Authority();
        AndroidCredentialBroker.Owner owner = authority.newOwner();
        try (AndroidCredentialBroker broker = new AndroidCredentialBroker(authority)) {
            byte[] expected = MessageDigest.getInstance("SHA-256").digest(fixture());
            check(!broker.consumeInspectionProof(owner.use, old, expected) && !broker.release(owner.manage, old));
            String fresh = broker.put(owner.manage, fixture());
            check(fresh != null && !old.equals(fresh) && broker.consumeInspectionProof(owner.use, fresh, expected));
        }
        checkSingleKey();
        check(getTargetContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().clear().commit());
    }
}
