package dev.patina.player;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;

import java.io.File;
import java.io.FileNotFoundException;

/**
 * Album covers for the car. Android Auto takes a browse row's picture only
 * as a content:// or android.resource:// URI, never a file path, so the
 * rows carry content://dev.patina.player.art/KEY and this hands over the
 * file. The key is one the Rust side gave out with the row (see carArt in
 * jni_glue.rs); any other key gets nothing, so this serves covers the car
 * was shown and no other file.
 *
 * Exported, as Google's sample music player does, since Android Auto's
 * package reads it directly.
 */
public final class ArtProvider extends ContentProvider {
    static final String AUTHORITY = "dev.patina.player.art";

    static Uri uriFor(String key) {
        return new Uri.Builder()
            .scheme("content")
            .authority(AUTHORITY)
            .appendPath(key)
            .build();
    }

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        if (mode != null && !mode.equals("r")) {
            throw new FileNotFoundException("read only");
        }
        String key = uri.getLastPathSegment();
        // The player may not be running yet when a car opens a list it
        // remembered: nothing to serve until it is.
        if (key == null || !Native.ready()) {
            throw new FileNotFoundException(String.valueOf(key));
        }
        String path = Native.carArt(key);
        if (path == null) {
            throw new FileNotFoundException(key);
        }
        return ParcelFileDescriptor.open(new File(path), ParcelFileDescriptor.MODE_READ_ONLY);
    }

    @Override
    public String getType(Uri uri) {
        return "image/png";
    }

    @Override
    public Cursor query(Uri uri, String[] projection, String selection, String[] args,
                        String sortOrder) {
        return null;
    }

    @Override
    public Uri insert(Uri uri, ContentValues values) {
        return null;
    }

    @Override
    public int delete(Uri uri, String selection, String[] args) {
        return 0;
    }

    @Override
    public int update(Uri uri, ContentValues values, String selection, String[] args) {
        return 0;
    }
}
