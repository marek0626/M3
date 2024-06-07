#!/bin/bash

parallel=1

sigusr1() {
    is_running=1
}

term() {
    count=$(jobs -p -r | wc -l)
    if [ "$count" -gt 0 ]; then
        echo "Terminating running jobs ($count)..."
        # kill the whole process group to kill also the childs in an easy and reliable way.
        for pid in $(jobs -p -r); do
            kill -INT "-$pid"
        done
    fi
}

sigint() {
    term
    exit 1
}

jobs_init() {
    parallel=$1

    # enable job control
    set -m

    # let the spawned jobs signal us
    trap sigusr1 USR1
    trap sigint INT TERM ERR
    trap term EXIT
}

jobs_submit() {
    # wait until there are free slots
    while [ "$(jobs -p -r | wc -l)" -ge $parallel ]; do
        sleep .1 || kill -INT $$
    done

    # start job
    is_running=0
    ( "$@" ) &

    # wait until it's started
    while [ $is_running -eq 0 ]; do
        sleep .1 || kill -INT $$
    done
}

jobs_started() {
    kill -USR1 $$
}

jobs_wait() {
    wait
}
