# bashrc-sort hook: show moves, ask about unsure blocks. Interactive TTY only.
if [[ $- == *i* && -t 0 && -x "$HOME/.cargo/bin/bashrc-sort" ]]; then
    # trap so Ctrl-C in the prompt doesn't abort the rest of shell startup
    trap ':' INT
    "$HOME/.cargo/bin/bashrc-sort" prompt
    trap - INT
fi
