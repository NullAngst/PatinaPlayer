package dev.patina.player;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
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
 */
public class NowPlayingWidget extends AppWidgetProvider {
    private static final String ACTION = "dev.patina.player.WIDGET";
    // What it last showed, for a widget added while something plays.
    private static String title = "", artist = "", artPath = "";
    private static int state = Native.STOPPED;
    private static String shownArt;
    private static Bitmap art;

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
        v.setImageViewResource(R.id.widget_play, state == Native.PLAYING
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
