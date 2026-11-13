package dev.patina.player;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.widget.RemoteViews;

/**
 * The home screen widget: the cover, what's playing, and back, play or
 * pause, and next. It follows what PlaybackService is told, so it always
 * shows the same as the notification.
 *
 * The buttons come back here as broadcasts. With the app running they go
 * to the player; with it closed (the phone restarted, Android stopped it)
 * there's no player to tell, so they open the app instead. A tap on the
 * cover or the title opens the app either way.
 *
 * Its colors are the app's theme, sent by the app whenever the theme
 * changes and kept in the app's preferences, so it stays in the theme
 * after a restart. Until the app has sent any, it's in Runout's.
 */
public class NowPlayingWidget extends AppWidgetProvider {
    private static final String ACTION = "dev.patina.player.WIDGET";
    // What it last showed, for a widget added while something plays.
    private static String title = "", artist = "", artPath = "";
    private static int state = Native.STOPPED;
    private static String shownArt;
    private static Bitmap art;

    // The theme's colors: surface, border, text, muted, accent, on-accent.
    private static final int SURFACE = 0, BORDER = 1, TEXT = 2, MUTED = 3, ACCENT = 4, ON_ACCENT = 5;
    private static final int[] RUNOUT = {
        0xFF132220, 0xFF2A3D39, 0xFFE6ECE9, 0xFF8FA39D, 0xFF5FB8A5, 0xFF0B1513,
    };
    private static final String PREFS = "widget";
    private static int[] colors;

    /** From the app, when its theme changes. Keeps them and redraws. */
    static void setColors(Context ctx, int[] c) {
        if (c == null || c.length != RUNOUT.length || java.util.Arrays.equals(c, colors)) {
            return;
        }
        colors = c;
        StringBuilder b = new StringBuilder();
        for (int i = 0; i < c.length; i++) {
            if (i > 0) {
                b.append(',');
            }
            b.append(c[i]);
        }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit().putString("colors", b.toString()).apply();
        push(ctx);
    }

    /** The colors to draw with: the last the app sent, or Runout's. */
    private static int[] colors(Context ctx) {
        if (colors != null) {
            return colors;
        }
        String saved = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .getString("colors", null);
        if (saved != null) {
            String[] parts = saved.split(",");
            if (parts.length == RUNOUT.length) {
                try {
                    int[] c = new int[parts.length];
                    for (int i = 0; i < parts.length; i++) {
                        c[i] = Integer.parseInt(parts[i]);
                    }
                    colors = c;
                    return c;
                } catch (NumberFormatException e) {
                    // Fall through to Runout's.
                }
            }
        }
        return RUNOUT;
    }

    /** A color without its alpha, for tinting. */
    private static int opaque(int c) {
        return c | 0xFF000000;
    }

    /** From PlaybackService.update, on the main thread. */
    static void show(Context ctx, String t, String ar, String artFile, int st) {
        boolean changed = st != state || !eq(t, title) || !eq(ar, artist) || !eq(artFile, artPath);
        title = t == null ? "" : t;
        artist = ar == null ? "" : ar;
        artPath = artFile == null ? "" : artFile;
        state = st;
        if (changed) {
            push(ctx);
        }
    }

    private static boolean eq(String a, String b) {
        return a == null ? b == null || b.isEmpty() : a.equals(b);
    }

    private static void push(Context ctx) {
        AppWidgetManager m = AppWidgetManager.getInstance(ctx);
        ComponentName me = new ComponentName(ctx, NowPlayingWidget.class);
        int[] ids = m.getAppWidgetIds(me);
        if (ids == null || ids.length == 0) {
            return;
        }
        m.updateAppWidget(me, views(ctx));
    }

    @Override
    public void onUpdate(Context ctx, AppWidgetManager m, int[] ids) {
        m.updateAppWidget(ids, views(ctx));
    }

    @Override
    public void onReceive(Context ctx, Intent i) {
        if (ACTION.equals(i.getAction())) {
            int cmd = i.getIntExtra("cmd", Native.TOGGLE);
            if (Native.ready()) {
                Native.command(cmd, 0);
            } else {
                open(ctx);
            }
            return;
        }
        super.onReceive(ctx, i);
    }

    private static void open(Context ctx) {
        Intent o = new Intent(ctx, MainActivity.class)
            .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        ctx.startActivity(o);
    }

    private static RemoteViews views(Context ctx) {
        RemoteViews v = new RemoteViews(ctx.getPackageName(), R.layout.widget_now_playing);
        int[] c = colors(ctx);
        // The ground and its edge are white shapes tinted to the theme: a
        // widget can't recolor a view's background before Android 12, but
        // it can tint an image on every version. A touch see-through, like
        // the launcher's own widgets.
        v.setInt(R.id.widget_bg, "setColorFilter", opaque(c[SURFACE]));
        v.setInt(R.id.widget_bg, "setImageAlpha", 0xF2);
        v.setInt(R.id.widget_edge, "setColorFilter", opaque(c[BORDER]));
        v.setTextColor(R.id.widget_title, c[TEXT]);
        v.setTextColor(R.id.widget_artist, c[MUTED]);
        v.setInt(R.id.widget_prev, "setColorFilter", opaque(c[TEXT]));
        v.setInt(R.id.widget_next, "setColorFilter", opaque(c[TEXT]));
        v.setInt(R.id.widget_play_disc, "setColorFilter", opaque(c[ACCENT]));
        v.setInt(R.id.widget_play_icon, "setColorFilter", opaque(c[ON_ACCENT]));
        boolean has = state != Native.STOPPED && !title.isEmpty();
        v.setTextViewText(R.id.widget_title, has ? title : "PatinaPlayer");
        v.setTextViewText(R.id.widget_artist, has ? artist : "Nothing playing");
        if (!artPath.equals(shownArt == null ? "" : shownArt)) {
            shownArt = artPath;
            art = artPath.isEmpty() ? null : small(artPath);
        }
        if (has && art != null) {
            v.setImageViewBitmap(R.id.widget_art, art);
        } else {
            v.setImageViewResource(R.id.widget_art, R.mipmap.ic_launcher);
        }
        v.setImageViewResource(R.id.widget_play_icon, state == Native.PLAYING
            ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play);
        v.setContentDescription(R.id.widget_play, state == Native.PLAYING ? "Pause" : "Play");
        v.setOnClickPendingIntent(R.id.widget_prev, button(ctx, Native.BACK));
        v.setOnClickPendingIntent(R.id.widget_play, button(ctx, Native.TOGGLE));
        v.setOnClickPendingIntent(R.id.widget_next, button(ctx, Native.NEXT));
        Intent o = new Intent(ctx, MainActivity.class)
            .setFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
        PendingIntent openApp = PendingIntent.getActivity(ctx, 100, o,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        v.setOnClickPendingIntent(R.id.widget_art, openApp);
        v.setOnClickPendingIntent(R.id.widget_text, openApp);
        return v;
    }

    private static PendingIntent button(Context ctx, int cmd) {
        Intent i = new Intent(ctx, NowPlayingWidget.class).setAction(ACTION).putExtra("cmd", cmd);
        return PendingIntent.getBroadcast(ctx, cmd, i,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    /**
     * The cover, scaled down: a widget's views cross to the launcher in one
     * parcel, and a full-size cover would be too big for it.
     */
    private static Bitmap small(String path) {
        BitmapFactory.Options o = new BitmapFactory.Options();
        o.inJustDecodeBounds = true;
        BitmapFactory.decodeFile(path, o);
        int side = Math.max(o.outWidth, o.outHeight);
        int sample = 1;
        while (side / (sample * 2) >= 256) {
            sample *= 2;
        }
        o = new BitmapFactory.Options();
        o.inSampleSize = sample;
        return BitmapFactory.decodeFile(path, o);
    }
}
