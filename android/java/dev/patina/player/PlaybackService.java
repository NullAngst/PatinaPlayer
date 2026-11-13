package dev.patina.player;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ServiceInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.drawable.Icon;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.media.MediaDescription;
import android.media.MediaMetadata;
import android.media.browse.MediaBrowser;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.Bundle;
import android.os.SystemClock;
import android.service.media.MediaBrowserService;

import java.util.ArrayList;
import java.util.List;

/**
 * Keeps the music going with the screen off or the app in the background:
 * a foreground service with the media notification, the media session the
 * lock screen, Bluetooth buttons and Android Auto talk to, audio focus,
 * and pausing when headphones come out. All of it follows what the Rust
 * player reports through {@link Native#onState}.
 *
 * It's a MediaBrowserService so Android Auto takes it for a music app and
 * drives its session: next and previous on the car's screen and wheel
 * buttons, instead of the generic controls (seek back and forward) it
 * gives an app it doesn't know. The car's tabs are Home (shuffle,
 * Favorites, Recently played, Recently added, Mixes), Artists, Albums,
 * Playlists and Folders; the tree comes from the Rust library
 * (patina_core::browse::auto) and picking a song there plays its list from
 * that song. Beside the transport the car shows a heart for the song
 * playing and a shuffle button.
 *
 * A car can bind this with the app closed. Then there's no player yet,
 * and this starts one without the window (Native.initWithoutWindow), so
 * the car browses and plays as it would with the app open.
 */
public class PlaybackService extends MediaBrowserService {
    private static final String CHANNEL = "playback";
    private static final int NOTE_ID = 1;
    private static final String ACTION = "dev.patina.player.ACTION";
    private static final String ROOT = "root";
    // Ask Android Auto to keep the next and previous slots for those two
    // buttons, never giving them to anything else. Strings from
    // androidx.media's MediaConstants, which this app doesn't link.
    private static final String RESERVE_NEXT =
        "android.media.playback.ALWAYS_RESERVE_SPACE_FOR.ACTION_SKIP_TO_NEXT";
    private static final String RESERVE_PREV =
        "android.media.playback.ALWAYS_RESERVE_SPACE_FOR.ACTION_SKIP_TO_PREVIOUS";
    // How the car lays rows out, and how many tabs it has room for. Also
    // from androidx.media's MediaConstants.
    private static final String STYLE_SUPPORTED = "android.media.browse.CONTENT_STYLE_SUPPORTED";
    private static final String STYLE_BROWSABLE = "android.media.browse.CONTENT_STYLE_BROWSABLE_HINT";
    private static final String STYLE_PLAYABLE = "android.media.browse.CONTENT_STYLE_PLAYABLE_HINT";
    private static final String STYLE_GROUP = "android.media.browse.CONTENT_STYLE_GROUP_TITLE_HINT";
    private static final String ROOT_LIMIT = "androidx.media.MediaBrowserCompat.Extras.KEY_ROOT_CHILDREN_LIMIT";
    private static final String SEARCH_SUPPORTED = "android.media.browse.SEARCH_SUPPORTED";
    private static final int STYLE_LIST = 1;
    private static final int STYLE_GRID = 2;
    // The car's own buttons beside play and pause.
    private static final String ACT_LOVE = "dev.patina.player.LOVE";
    private static final String ACT_SHUFFLE = "dev.patina.player.SHUFFLE";

    private static PlaybackService running;
    // The latest state, kept for when the service is (re)created.
    private static String title = "", artist = "", album = "", artPath = "";
    private static long duration, position, positionAt;
    private static int state = Native.STOPPED;
    // 0 can't be hearted, 1 not hearted, 2 hearted.
    private static int loved;
    private static boolean shuffle;

    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focus;
    private boolean focusHeld;
    private boolean resumeOnGain;
    private String shownArt;
    private Bitmap art;
    // Started (not just bound by a car), so it outlives the car unbinding.
    private boolean started;

    private final BroadcastReceiver noisy = new BroadcastReceiver() {
        @Override
        public void onReceive(Context c, Intent i) {
            // Headphones out: pause instead of playing out loud.
            if (AudioManager.ACTION_AUDIO_BECOMING_NOISY.equals(i.getAction())) {
                Native.command(Native.PAUSE, 0);
            }
        }
    };

    /** From Native.onState, on the main thread. */
    static void update(Context ctx, String t, String ar, String al, String artFile,
                       long durMs, long posMs, int st, int love, boolean shuf) {
        boolean wasStopped = state == Native.STOPPED;
        title = t; artist = ar; album = al; artPath = artFile;
        loved = love; shuffle = shuf;
        duration = durMs; position = posMs; positionAt = SystemClock.elapsedRealtime();
        state = st;
        NowPlayingWidget.show(ctx, t, ar, artFile, st);
        if (running != null) {
            running.refresh();
        } else if (st == Native.PLAYING || (wasStopped && st == Native.PAUSED)) {
            // Starting playback, from the app in front, which Android allows.
            Intent i = new Intent(ctx, PlaybackService.class);
            if (Build.VERSION.SDK_INT >= 26) {
                ctx.startForegroundService(i);
            } else {
                ctx.startService(i);
            }
        }
    }

    static void stop(Context ctx) {
        state = Native.STOPPED;
        NowPlayingWidget.show(ctx, title, artist, artPath, Native.STOPPED);
        if (running != null) {
            running.stopForeground(STOP_FOREGROUND_REMOVE);
            running.stopSelf();
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        running = this;
        // Started by a car with the app closed: no player yet. Start one
        // without the window.
        if (!Native.ready()) {
            try {
                Native.initWithoutWindow(this);
            } catch (Throwable t) {
                android.util.Log.e("PatinaPlayer", "can't start the player for the car", t);
            }
        }
        audio = (AudioManager) getSystemService(AUDIO_SERVICE);
        NotificationManager nm = getSystemService(NotificationManager.class);
        NotificationChannel ch = new NotificationChannel(CHANNEL, "Playback",
            NotificationManager.IMPORTANCE_LOW);
        ch.setShowBadge(false);
        nm.createNotificationChannel(ch);

        session = new MediaSession(this, "PatinaPlayer");
        session.setCallback(new MediaSession.Callback() {
            @Override public void onPlay() { Native.command(Native.PLAY, 0); }
            @Override public void onPause() { Native.command(Native.PAUSE, 0); }
            @Override public void onSkipToNext() { Native.command(Native.NEXT, 0); }
            @Override public void onSkipToPrevious() { Native.command(Native.BACK, 0); }
            @Override public void onStop() { Native.command(Native.STOP, 0); }
            @Override public void onSeekTo(long pos) { Native.command(Native.SEEK, pos); }
            // Some cars send fast forward and rewind from the wheel's
            // skip buttons. The app has no seeking by steps, so they skip.
            @Override public void onFastForward() { Native.command(Native.NEXT, 0); }
            @Override public void onRewind() { Native.command(Native.BACK, 0); }
            // Picked in the car's browse list.
            @Override public void onPlayFromMediaId(String id, Bundle extras) {
                if (Native.ready() && id != null) {
                    Native.playMediaId(id);
                }
            }
            // "Play Nirvana on PatinaPlayer". Nothing said shuffles it all.
            @Override public void onPlayFromSearch(String query, Bundle extras) {
                if (Native.ready()) {
                    Native.playMediaId("search:" + (query == null ? "" : query));
                }
            }
            // The car's heart and shuffle buttons.
            @Override public void onCustomAction(String action, Bundle extras) {
                if (ACT_LOVE.equals(action)) {
                    Native.command(Native.LOVE, 0);
                } else if (ACT_SHUFFLE.equals(action)) {
                    Native.command(Native.SHUFFLE, 0);
                }
            }
        });
        Bundle extras = new Bundle();
        extras.putBoolean(RESERVE_NEXT, true);
        extras.putBoolean(RESERVE_PREV, true);
        session.setExtras(extras);
        session.setActive(true);
        // Android Auto and other media browsers reach the session through this.
        setSessionToken(session.getSessionToken());
        registerReceiver(noisy, new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY));

        AudioAttributes attrs = new AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build();
        focus = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(attrs)
            // Let Android lower the volume for a notification sound itself.
            .setWillPauseWhenDucked(false)
            .setOnAudioFocusChangeListener(this::onFocus)
            .build();
    }

    private void onFocus(int change) {
        switch (change) {
            case AudioManager.AUDIOFOCUS_LOSS:
                // Another player took over for good.
                focusHeld = false;
                resumeOnGain = false;
                Native.command(Native.PAUSE, 0);
                break;
            case AudioManager.AUDIOFOCUS_LOSS_TRANSIENT:
                // A call, a navigation prompt: pause, and come back after.
                resumeOnGain = state == Native.PLAYING;
                Native.command(Native.PAUSE, 0);
                break;
            case AudioManager.AUDIOFOCUS_GAIN:
                if (resumeOnGain) {
                    Native.command(Native.PLAY, 0);
                }
                resumeOnGain = false;
                break;
            default:
                break;
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        started = true;
        if (intent != null && ACTION.equals(intent.getAction())) {
            Native.command(intent.getIntExtra("cmd", Native.TOGGLE), 0);
        }
        refresh();
        return START_NOT_STICKY;
    }

    private void refresh() {
        if (state == Native.PLAYING && !focusHeld) {
            focusHeld = audio.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
        } else if (state == Native.STOPPED && focusHeld) {
            audio.abandonAudioFocusRequest(focus);
            focusHeld = false;
        }
        if (artPath == null || !artPath.equals(shownArt)) {
            shownArt = artPath;
            art = artPath == null || artPath.isEmpty() ? null : BitmapFactory.decodeFile(artPath);
        }
        MediaMetadata.Builder meta = new MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
            .putString(MediaMetadata.METADATA_KEY_ALBUM, album)
            .putLong(MediaMetadata.METADATA_KEY_DURATION, duration);
        if (art != null) {
            meta.putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, art);
        }
        session.setMetadata(meta.build());
        int ps = state == Native.PLAYING ? PlaybackState.STATE_PLAYING
            : state == Native.PAUSED ? PlaybackState.STATE_PAUSED
            : PlaybackState.STATE_STOPPED;
        PlaybackState.Builder pb = new PlaybackState.Builder()
            .setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
                | PlaybackState.ACTION_PLAY_PAUSE | PlaybackState.ACTION_SKIP_TO_NEXT
                | PlaybackState.ACTION_SKIP_TO_PREVIOUS | PlaybackState.ACTION_STOP
                | PlaybackState.ACTION_PLAY_FROM_MEDIA_ID
                | PlaybackState.ACTION_PLAY_FROM_SEARCH
                | (duration > 0 ? PlaybackState.ACTION_SEEK_TO : 0))
            .setState(ps, position, state == Native.PLAYING ? 1f : 0f, positionAt);
        // The car shows these beside the transport: heart first, then
        // shuffle, each drawn as it is now.
        if (loved > 0) {
            pb.addCustomAction(new PlaybackState.CustomAction.Builder(ACT_LOVE,
                loved == 2 ? "Unheart" : "Heart",
                loved == 2 ? R.drawable.ic_action_heart_on : R.drawable.ic_action_heart).build());
        }
        if (state != Native.STOPPED) {
            pb.addCustomAction(new PlaybackState.CustomAction.Builder(ACT_SHUFFLE,
                shuffle ? "Shuffle on" : "Shuffle off",
                shuffle ? R.drawable.ic_action_shuffle_on : R.drawable.ic_action_shuffle).build());
        }
        session.setPlaybackState(pb.build());

        Notification n = notification();
        if (state == Native.STOPPED) {
            stopForeground(STOP_FOREGROUND_REMOVE);
            stopSelf();
        } else if (state == Native.PLAYING) {
            // A car only binds the service. Start it too, so it keeps
            // playing when the car lets go; the car's command is what
            // lets it start from the background.
            if (!started) {
                started = true;
                Intent self = new Intent(this, PlaybackService.class);
                try {
                    if (Build.VERSION.SDK_INT >= 26) {
                        startForegroundService(self);
                    } else {
                        startService(self);
                    }
                } catch (RuntimeException e) {
                    started = false;
                }
            }
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
            } else {
                startForeground(NOTE_ID, n);
            }
        } else {
            // Paused: the notification stays, so it can be resumed from
            // there, but Android may stop the app if it needs the memory.
            stopForeground(STOP_FOREGROUND_DETACH);
            getSystemService(NotificationManager.class).notify(NOTE_ID, n);
        }
    }

    private PendingIntent action(int cmd) {
        Intent i = new Intent(this, PlaybackService.class).setAction(ACTION).putExtra("cmd", cmd);
        return PendingIntent.getService(this, cmd, i,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private Notification.Action button(int icon, String label, int cmd) {
        return new Notification.Action.Builder(
            Icon.createWithResource(this, icon), label, action(cmd)).build();
    }

    private Notification notification() {
        Intent open = new Intent(this, MainActivity.class)
            .setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_NEW_TASK);
        PendingIntent content = PendingIntent.getActivity(this, 0, open,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        boolean playing = state == Native.PLAYING;
        Notification.Builder b = new Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_patina)
            .setContentTitle(title)
            .setContentText(artist)
            .setSubText(album)
            .setLargeIcon(art)
            .setContentIntent(content)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOngoing(playing)
            .setShowWhen(false)
            .addAction(button(android.R.drawable.ic_media_previous, "Back", Native.BACK))
            .addAction(button(
                playing ? android.R.drawable.ic_media_pause : android.R.drawable.ic_media_play,
                playing ? "Pause" : "Play", Native.TOGGLE))
            .addAction(button(android.R.drawable.ic_media_next, "Next", Native.NEXT))
            .setStyle(new Notification.MediaStyle()
                .setMediaSession(session.getSessionToken())
                .setShowActionsInCompactView(0, 1, 2));
        return b.build();
    }

    @Override
    public void onDestroy() {
        running = null;
        unregisterReceiver(noisy);
        if (focusHeld) {
            audio.abandonAudioFocusRequest(focus);
        }
        session.setActive(false);
        session.release();
        super.onDestroy();
    }

    // ----- Media browsing (Android Auto) -----

    @Override
    public BrowserRoot onGetRoot(String clientPackage, int clientUid, Bundle hints) {
        Bundle extras = new Bundle();
        extras.putBoolean(STYLE_SUPPORTED, true);
        extras.putInt(STYLE_BROWSABLE, STYLE_LIST);
        extras.putInt(STYLE_PLAYABLE, STYLE_LIST);
        extras.putBoolean(SEARCH_SUPPORTED, false);
        // The tabs: five, or four with Home folded into Playlists when the
        // car says it has room for only four.
        int limit = hints == null ? 0 : hints.getInt(ROOT_LIMIT, 0);
        return new BrowserRoot(limit > 0 && limit < 5 ? "root4" : ROOT, extras);
    }

    @Override
    public void onLoadChildren(String parentId, Result<List<MediaBrowser.MediaItem>> result) {
        if (!Native.ready()) {
            // The player couldn't be started without the window (see
            // onCreate): there's nothing to browse.
            List<MediaBrowser.MediaItem> none = new ArrayList<>();
            if (parentId != null && parentId.startsWith(ROOT)) {
                none.add(new MediaBrowser.MediaItem(new MediaDescription.Builder()
                    .setMediaId("closed")
                    .setTitle("Open PatinaPlayer on the phone")
                    .setSubtitle("Its library shows here once it's running")
                    .build(), 0));
            }
            result.sendResult(none);
            return;
        }
        // The library is read off the main thread; a big smart playlist
        // takes a moment.
        result.detach();
        new Thread(() -> {
            List<MediaBrowser.MediaItem> items = new ArrayList<>();
            String[] raw = null;
            try {
                raw = Native.browse(parentId);
            } catch (Throwable t) {
                raw = null;
            }
            if (raw != null) {
                for (int i = 0; i + 4 < raw.length; i += 5) {
                    MediaDescription.Builder d = new MediaDescription.Builder()
                        .setMediaId(raw[i])
                        .setTitle(raw[i + 1]);
                    if (raw[i + 2] != null && !raw[i + 2].isEmpty()) {
                        d.setSubtitle(raw[i + 2]);
                    }
                    String f = raw[i + 3] == null ? "" : raw[i + 3];
                    Bundle extras = new Bundle();
                    if (f.contains("g")) {
                        extras.putInt(STYLE_BROWSABLE, STYLE_GRID);
                        extras.putInt(STYLE_PLAYABLE, STYLE_LIST);
                    }
                    if (raw[i + 4] != null && !raw[i + 4].isEmpty()) {
                        extras.putString(STYLE_GROUP, raw[i + 4]);
                    }
                    if (!extras.isEmpty()) {
                        d.setExtras(extras);
                    }
                    int flags = 0;
                    if (f.contains("b")) {
                        flags |= MediaBrowser.MediaItem.FLAG_BROWSABLE;
                    }
                    if (f.contains("p")) {
                        flags |= MediaBrowser.MediaItem.FLAG_PLAYABLE;
                    }
                    items.add(new MediaBrowser.MediaItem(d.build(), flags));
                }
            }
            result.sendResult(items);
        }, "patina-car-browse").start();
    }
}
