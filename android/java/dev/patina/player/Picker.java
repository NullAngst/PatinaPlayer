package dev.patina.player;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.os.storage.StorageManager;
import android.os.storage.StorageVolume;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * The system's own pickers (DocumentsUI): a music folder for the library,
 * and an EQ file to import. Rust asks through Native.pickFolder and
 * Native.pickFile and waits; the answer goes back through Native.pickDone.
 *
 * The library scans folders by path, the way it does on Linux, so a picked
 * folder is turned back into one. That works for the phone's storage and
 * SD cards, which is where music lives; a folder in some app's cloud
 * provider has no path, and the answer says so.
 */
final class Picker {
    /** Request codes are this plus what's being picked. */
    private static final int REQUEST_BASE = 100;

    private static final String STORAGE = "com.android.externalstorage.documents";
    private static final String DOWNLOADS = "com.android.providers.downloads.documents";

    private Picker() {}

    static void start(Activity a, int what) {
        if (a == null || a.isFinishing()) {
            Native.pickDone(what, null, "The app isn't on screen to open the picker");
            return;
        }
        Intent i;
        if (what == Native.PICK_FOLDER) {
            i = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
            // Start in Music, where most phones keep it.
            i.putExtra(DocumentsContract.EXTRA_INITIAL_URI,
                DocumentsContract.buildDocumentUri(STORAGE, "primary:" + Environment.DIRECTORY_MUSIC));
        } else {
            i = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            // AutoEq and Equalizer APO files are .txt, which some providers
            // call octet-stream.
            i.setType("*/*");
            i.putExtra(Intent.EXTRA_MIME_TYPES,
                new String[] {"text/*", "application/octet-stream"});
        }
        try {
            a.startActivityForResult(i, REQUEST_BASE + what);
        } catch (ActivityNotFoundException e) {
            Native.pickDone(what, null,
                "This phone has no picker for that. Use \"Type a path\" instead.");
        }
    }

    /** A result from the activity: true if it was a pick, and answered. */
    static boolean onResult(Activity a, int request, int result, Intent data) {
        int what = request - REQUEST_BASE;
        if (what != Native.PICK_FOLDER && what != Native.PICK_EQ_FILE) {
            return false;
        }
        Uri uri = result == Activity.RESULT_OK && data != null ? data.getData() : null;
        if (uri == null) {
            Native.pickDone(what, null, null);
            return true;
        }
        if (what == Native.PICK_FOLDER) {
            String path = treePath(a, uri);
            if (path == null) {
                Native.pickDone(what, null,
                    "That folder isn't on the phone's storage or an SD card, so it has no path "
                    + "to scan. Pick a folder there, or add a music server.");
            } else {
                Native.pickDone(what, path, null);
            }
        } else {
            File f = copyToCache(a, uri);
            if (f == null) {
                Native.pickDone(what, null, "Couldn't read that file");
            } else {
                Native.pickDone(what, f.getPath(), null);
            }
        }
        return true;
    }

    /** The filesystem path of a folder picked in DocumentsUI, or null. */
    static String treePath(Context c, Uri tree) {
        String id;
        try {
            id = DocumentsContract.getTreeDocumentId(tree);
        } catch (IllegalArgumentException e) {
            return null;
        }
        String auth = tree.getAuthority();
        if (STORAGE.equals(auth)) {
            // "primary:Music/Albums", or "1A2B-3C4D:Music" on an SD card.
            int colon = id.indexOf(':');
            String volume = colon < 0 ? id : id.substring(0, colon);
            String rest = colon < 0 ? "" : id.substring(colon + 1);
            File base = volumeDir(c, volume);
            if (base == null) {
                return null;
            }
            return rest.isEmpty() ? base.getPath() : new File(base, rest).getPath();
        }
        if (DOWNLOADS.equals(auth)) {
            if (id.startsWith("raw:")) {
                return id.substring(4);
            }
            if (id.equals("downloads")) {
                return Environment.getExternalStoragePublicDirectory(
                    Environment.DIRECTORY_DOWNLOADS).getPath();
            }
        }
        return null;
    }

    /** Where a storage volume is mounted: the phone's own, or a card's. */
    private static File volumeDir(Context c, String volume) {
        if ("primary".equalsIgnoreCase(volume)) {
            return Environment.getExternalStorageDirectory();
        }
        if (Build.VERSION.SDK_INT >= 30) {
            StorageManager sm = c.getSystemService(StorageManager.class);
            if (sm != null) {
                for (StorageVolume v : sm.getStorageVolumes()) {
                    if (volume.equalsIgnoreCase(v.getUuid()) && v.getDirectory() != null) {
                        return v.getDirectory();
                    }
                }
            }
        }
        // Cards mount under their id on every Android this app runs on.
        File guess = new File("/storage/" + volume);
        return guess.exists() ? guess : null;
    }

    /** Copy a picked file into the app's cache, where Rust can read it. */
    private static File copyToCache(Context c, Uri uri) {
        String name = "eq-import.txt";
        try (Cursor cur = c.getContentResolver().query(uri,
                new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cur != null && cur.moveToFirst() && !cur.isNull(0)) {
                // Keep the name (ParametricEQ.txt, GraphicEQ.txt), minus
                // anything that could make it a path.
                name = cur.getString(0).replaceAll("[/\\\\]", "_");
            }
        } catch (RuntimeException e) {
            // The name is only a nicety.
        }
        File dir = new File(c.getCacheDir(), "picked");
        if (!dir.isDirectory() && !dir.mkdirs()) {
            return null;
        }
        File out = new File(dir, name);
        try (InputStream in = c.getContentResolver().openInputStream(uri);
             OutputStream o = new FileOutputStream(out)) {
            if (in == null) {
                return null;
            }
            byte[] buf = new byte[16384];
            // EQ files are a few KB; anything past 4 MB isn't one.
            long total = 0;
            int n;
            while ((n = in.read(buf)) > 0) {
                total += n;
                if (total > 4L << 20) {
                    return null;
                }
                o.write(buf, 0, n);
            }
            return out;
        } catch (Exception e) {
            return null;
        }
    }
}
