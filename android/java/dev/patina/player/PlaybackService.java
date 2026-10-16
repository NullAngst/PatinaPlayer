package dev.patina.player;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
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
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.IBinder;
import android.os.SystemClock;

/**
 * Keeps the music going with the screen off or the app in the background:
 * a foreground service with the media notification, the media session the
 * lock screen, Bluetooth buttons and Android Auto talk to, audio focus,
 * and pausing when headphones come out. All of it follows what the Rust
 * player reports through {@link Native#onState}.
 */
public class PlaybackService extends Service {
    private static final String CHANNEL = "playback";
    private static final int NOTE_ID = 1;
    private static final String ACTION = "dev.patina.player.ACTION";

    private static PlaybackService running;
    // The latest state, kept for when the service is (re)created.
    private static String title = "", artist = "", album = "", artPath = "";
    private static long duration, position, positionAt;
    private static int state = Native.STOPPED;

    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focus;
    private boolean focusHeld;
    private boolean resumeOnGain;
    private String shownArt;
    private Bitmap art;

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
                       long durMs, long posMs, int st) {
        boolean wasStopped = state == Native.STOPPED;
        title = t; artist = ar; album = al; artPath = artFile;
        duration = durMs; position = posMs; positionAt = SystemClock.elapsedRealtime();
        state = st;
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
        if (running != null) {
            running.stopForeground(STOP_FOREGROUND_REMOVE);
            running.stopSelf();
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        running = this;
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
        });
        session.setActive(true);
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
        session.setPlaybackState(new PlaybackState.Builder()
            .setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
                | PlaybackState.ACTION_PLAY_PAUSE | PlaybackState.ACTION_SKIP_TO_NEXT
                | PlaybackState.ACTION_SKIP_TO_PREVIOUS | PlaybackState.ACTION_STOP
                | (duration > 0 ? PlaybackState.ACTION_SEEK_TO : 0))
            .setState(ps, position, state == Native.PLAYING ? 1f : 0f, positionAt)
            .build());

        Notification n = notification();
        if (state == Native.STOPPED) {
            stopForeground(STOP_FOREGROUND_REMOVE);
            stopSelf();
        } else if (state == Native.PLAYING) {
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

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
