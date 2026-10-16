package dev.patina.player;

import android.content.Context;
import android.media.AudioAttributes;
import android.media.AudioDeviceInfo;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioMixerAttributes;
import android.os.Build;

import java.util.HashSet;
import java.util.List;
import java.util.Set;

/**
 * Bit-perfect USB output, Android 14 and later. The system can run a USB
 * DAC's mixer in a bit-perfect mode: one app's stream goes to the DAC at
 * its own rate and depth, with no mixing, resampling or volume. This asks
 * for that mode at the song's format; the Rust side then opens a stream at
 * exactly that format.
 */
final class BitPerfect {
    /** Devices we put into bit-perfect mode, to put back afterwards. */
    private static final Set<Integer> set = new HashSet<>();

    private BitPerfect() {}

    static boolean isUsb(AudioDeviceInfo d) {
        int t = d.getType();
        return t == AudioDeviceInfo.TYPE_USB_DEVICE || t == AudioDeviceInfo.TYPE_USB_HEADSET;
    }

    private static AudioAttributes media() {
        return new AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
            .build();
    }

    /** bits: 16, 24 or 32 for integers, 0 for float. rate 0 clears. */
    static synchronized String set(Context ctx, int deviceId, int rate, int bits, int channels) {
        if (Build.VERSION.SDK_INT < 34) {
            return rate == 0 ? null : "bit-perfect needs Android 14 or later";
        }
        AudioManager am = (AudioManager) ctx.getSystemService(Context.AUDIO_SERVICE);
        if (rate == 0) {
            clear(am, deviceId);
            return null;
        }
        AudioDeviceInfo dev = find(am, deviceId);
        if (dev == null) {
            return "the USB DAC isn't connected";
        }
        String name = dev.getProductName() == null ? "the USB DAC" : dev.getProductName().toString();
        int encoding;
        switch (bits) {
            case 16: encoding = AudioFormat.ENCODING_PCM_16BIT; break;
            case 24: encoding = AudioFormat.ENCODING_PCM_24BIT_PACKED; break;
            case 32: encoding = AudioFormat.ENCODING_PCM_32BIT; break;
            default: encoding = AudioFormat.ENCODING_PCM_FLOAT; break;
        }
        List<AudioMixerAttributes> offered = am.getSupportedMixerAttributes(dev);
        boolean anyBitPerfect = false;
        for (AudioMixerAttributes m : offered) {
            if (m.getMixerBehavior() != AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT) {
                continue;
            }
            anyBitPerfect = true;
            AudioFormat f = m.getFormat();
            if (f.getSampleRate() == rate && f.getEncoding() == encoding
                    && f.getChannelCount() == channels) {
                if (am.setPreferredMixerAttributes(media(), dev, m)) {
                    set.add(dev.getId());
                    return null;
                }
                return "Android refused bit-perfect on " + name;
            }
        }
        if (!anyBitPerfect) {
            return "Android doesn't offer bit-perfect on " + name;
        }
        String depth = bits == 0 ? "float" : bits + "-bit";
        return name + " doesn't offer bit-perfect at " + depth + " " + rate + " Hz";
    }

    private static void clear(AudioManager am, int deviceId) {
        for (Integer id : new HashSet<>(set)) {
            if (deviceId >= 0 && id != deviceId) {
                continue;
            }
            AudioDeviceInfo dev = find(am, id);
            if (dev != null) {
                am.clearPreferredMixerAttributes(media(), dev);
            }
            set.remove(id);
        }
    }

    private static AudioDeviceInfo find(AudioManager am, int id) {
        for (AudioDeviceInfo d : am.getDevices(AudioManager.GET_DEVICES_OUTPUTS)) {
            if (d.getId() == id && isUsb(d)) {
                return d;
            }
        }
        return null;
    }
}
