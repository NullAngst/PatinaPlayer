package dev.patina.player;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Context;
import android.content.DialogInterface;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.ContextThemeWrapper;
import android.view.Gravity;
import android.view.Window;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/**
 * The app's text dialogs as Android's own: adding a server, a station or a
 * folder path, naming a playlist, and search. Slint's text field on Android
 * loses track of the keyboard (text typed twice over, no scrolling, the
 * field left under the keyboard); an EditText in a dialog has none of that
 * and moves clear of the keyboard by itself.
 *
 * Rust opens one with Native.prompt and hears back through
 * Native.promptDone (the fields' text, or null for cancel) and, for search,
 * Native.promptEdited as each letter is typed.
 */
final class Prompts {
    private Prompts() {}

    static void show(Activity a, long id, String title, String message, String[] labels,
                     String[] values, boolean[] secret, String ok, boolean live, boolean light) {
        if (a == null || a.isFinishing()) {
            Native.promptDone(id, null);
            return;
        }
        int theme = light
            ? android.R.style.Theme_DeviceDefault_Light_Dialog_Alert
            : android.R.style.Theme_DeviceDefault_Dialog_Alert;
        Context ctx = new ContextThemeWrapper(a, theme);
        float dp = a.getResources().getDisplayMetrics().density;

        LinearLayout box = new LinearLayout(ctx);
        box.setOrientation(LinearLayout.VERTICAL);
        int side = Math.round(22 * dp);
        box.setPadding(side, Math.round(8 * dp), side, Math.round(4 * dp));

        final EditText[] edits = new EditText[values.length];
        for (int i = 0; i < values.length; i++) {
            String label = labels[i] == null ? "" : labels[i];
            // Search has room for one line only: its label is the hint.
            if (!live && !label.isEmpty()) {
                TextView l = new TextView(ctx);
                l.setText(label);
                l.setTextSize(13);
                l.setPadding(0, i == 0 ? 0 : Math.round(14 * dp), 0, 0);
                box.addView(l);
            }
            EditText e = new EditText(ctx);
            e.setSingleLine(true);
            e.setInputType(inputType(label, secret[i]));
            if (live) {
                e.setHint(label);
            }
            e.setText(values[i] == null ? "" : values[i]);
            e.setSelection(e.getText().length());
            boolean last = i == values.length - 1;
            e.setImeOptions((last ? EditorInfo.IME_ACTION_DONE : EditorInfo.IME_ACTION_NEXT)
                | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
            if (live) {
                final int field = i;
                e.addTextChangedListener(new TextWatcher() {
                    @Override
                    public void beforeTextChanged(CharSequence s, int start, int count, int after) {}

                    @Override
                    public void onTextChanged(CharSequence s, int start, int before, int count) {}

                    @Override
                    public void afterTextChanged(Editable s) {
                        Native.promptEdited(id, field, s.toString());
                    }
                });
            }
            box.addView(e, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT));
            edits[i] = e;
        }
        ScrollView scroll = new ScrollView(ctx);
        scroll.addView(box);

        // Answered once, whichever way it closes.
        final boolean[] answered = {false};
        AlertDialog.Builder b = new AlertDialog.Builder(a, theme)
            .setTitle(title)
            .setView(scroll)
            .setPositiveButton(ok, (d, w) -> answer(id, edits, answered))
            // Search: Clear drops it; closing it any other way keeps it.
            .setNegativeButton(live ? "Clear" : "Cancel", (d, w) -> cancel(id, answered));
        if (message != null && !message.isEmpty()) {
            b.setMessage(message);
        }
        final AlertDialog dialog = b.create();
        dialog.setOnDismissListener((DialogInterface d) -> {
            if (live) {
                answer(id, edits, answered);
            } else {
                cancel(id, answered);
            }
        });
        // Done on the keyboard confirms, like the button.
        edits[edits.length - 1].setOnEditorActionListener((v, action, ev) -> {
            if (action == EditorInfo.IME_ACTION_DONE) {
                answer(id, edits, answered);
                dialog.dismiss();
                return true;
            }
            return false;
        });
        Window w = dialog.getWindow();
        if (w != null) {
            w.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE
                | WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
            if (live) {
                // At the top, barely dimmed: the results show underneath.
                w.setGravity(Gravity.TOP);
                w.setDimAmount(0.15f);
            }
        }
        dialog.show();
        edits[0].requestFocus();
    }

    /** Text, a password, or something like a URL or path (no autocorrect). */
    private static int inputType(String label, boolean secret) {
        if (secret) {
            return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD;
        }
        String l = label.toLowerCase();
        if (l.contains("url") || l.contains("address") || l.contains("path") || l.contains("folder")) {
            return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI;
        }
        if (l.contains("user name")) {
            return InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS;
        }
        return InputType.TYPE_CLASS_TEXT;
    }

    private static void answer(long id, EditText[] edits, boolean[] answered) {
        if (answered[0]) {
            return;
        }
        answered[0] = true;
        String[] out = new String[edits.length];
        for (int i = 0; i < edits.length; i++) {
            out[i] = edits[i].getText().toString();
        }
        Native.promptDone(id, out);
    }

    private static void cancel(long id, boolean[] answered) {
        if (answered[0]) {
            return;
        }
        answered[0] = true;
        Native.promptDone(id, null);
    }
}
