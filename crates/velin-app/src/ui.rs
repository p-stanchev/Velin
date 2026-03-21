slint::slint! {
    import { HorizontalBox, ScrollView, VerticalBox } from "std-widgets.slint";

    component FlatButton inherits Rectangle {
        in property <string> text;
        in property <bool> clickable: true;
        in property <bool> active: false;
        in property <bool> dark-mode: true;
        callback clicked();

        min-height: 34px;
        border-radius: 9px;
        border-width: 1px;
        border-color: active ? (dark-mode ? #647560 : #7d8f7b)
            : !clickable ? (dark-mode ? #2f2f2f : #d8d5cf)
            : (dark-mode ? #363636 : #cdc8bf);
        background: active ? (dark-mode ? #232923 : #eef3ec)
            : !clickable ? (dark-mode ? #1a1a1a : #f1f0ec)
            : (dark-mode ? #202020 : #faf8f3);

        Text {
            x: (parent.width - self.width) / 2;
            y: (parent.height - self.height) / 2;
            text: root.text;
            color: !root.clickable ? (root.dark-mode ? #7e7e7e : #9e9b95)
                : (root.dark-mode ? #f0f0f0 : #1a1a1a);
            font-size: 12px;
        }

        TouchArea {
            enabled: root.clickable;
            clicked => { root.clicked(); }
        }
    }

    component FlatInput inherits Rectangle {
        in-out property <string> text;
        in property <string> placeholder;
        in property <bool> enabled: true;
        in property <bool> dark-mode: true;
        callback edited();

        height: 38px;
        border-radius: 9px;
        border-width: 1px;
        border-color: dark-mode ? #343434 : #cbc7be;
        background: enabled ? (dark-mode ? #1c1c1c : #ffffff) : (dark-mode ? #181818 : #f1efea);

        input := TextInput {
            x: 12px;
            y: 12px;
            width: parent.width - 24px;
            text <=> root.text;
            color: root.dark-mode ? #f2f2f2 : #171717;
            selection-foreground-color: root.dark-mode ? #111111 : #ffffff;
            selection-background-color: root.dark-mode ? #f0f0f0 : #171717;
            enabled: root.enabled;
            single-line: true;
            font-size: 13px;
            edited => { root.edited(); }
        }

        if root.text == "" : Text {
            x: 12px;
            y: 14px;
            text: root.placeholder;
            color: root.dark-mode ? #777777 : #8a857d;
            font-size: 13px;
        }
    }

    export component AppWindow inherits Window {
        in-out property <string> target-ip: "127.0.0.1";
        in-out property <string> bind-ip: "0.0.0.0";
        in-out property <[string]> bind-ip-options: ["Automatic"];
        in-out property <string> bind-ip-selection: "Automatic";
        in-out property <[string]> output-device-options: ["System Default"];
        in-out property <string> output-device-selection: "System Default";
        in-out property <[string]> discovered-peer-options: ["No receivers found"];
        in-out property <string> discovered-peer-selection: "No receivers found";
        in-out property <string> control-port: "49000";
        in-out property <string> audio-port: "49001";
        in-out property <string> status-text: "Idle";
        in-out property <string> metrics-text: "No active stream.\n";
        in-out property <string> log-text: "";
        in-out property <string> local-addresses: "Local IP unavailable";
        in-out property <string> local-machine-name: "Unknown";
        in-out property <string> local-primary-ip: "Unavailable";
        in-out property <int> active-tab: 0;
        in-out property <bool> running: false;
        in-out property <bool> dark-mode: true;
        in-out property <bool> sender-mode: false;
        in-out property <bool> bind-ip-menu-open: false;
        in-out property <bool> output-device-menu-open: false;
        in-out property <bool> discovered-peer-menu-open: false;
        in-out property <bool> muted: false;
        callback select-session-tab();
        callback select-metrics-tab();
        callback select-settings-tab();
        callback start-target();
        callback start-source(string);
        callback stop-session();
        callback toggle-mute();
        callback refresh-discovery();
        callback save-settings(string, string, string, string, string, bool);
        callback choose-discovered-peer(string);
        callback report-bug();

        title: "Velin";
        icon: @image-url("../../assets/logo.svg");
        width: 680px;
        height: 700px;

        in property <color> bg: root.dark-mode ? #171717 : #f4f2ed;
        in property <color> panel-bg: root.dark-mode ? #1f1f1f : #fbfaf7;
        in property <color> panel-alt: root.dark-mode ? #1a1a1a : #f7f4ee;
        in property <color> border: root.dark-mode ? #2c2c2c : #d4d0c8;
        in property <color> text-primary: root.dark-mode ? #f1f1f1 : #1a1a1a;
        in property <color> text-secondary: root.dark-mode ? #a3a3a3 : #605b54;
        in property <color> text-tertiary: root.dark-mode ? #c7c7c7 : #4a453f;
        in property <color> accent: root.dark-mode ? #7a8d76 : #60715c;

        Rectangle {
            background: root.bg;

            VerticalBox {
                padding: 16px;
                spacing: 10px;

                HorizontalBox {
                    spacing: 8px;
                    height: 34px;

                    Rectangle { background: transparent; }

                    Rectangle {
                        width: 120px;
                        background: transparent;

                        FlatButton {
                            text: "Session";
                            dark-mode: root.dark-mode;
                            active: root.active-tab == 0;
                            clickable: root.active-tab != 0;
                            clicked => {
                                root.bind-ip-menu-open = false;
                                root.output-device-menu-open = false;
                                root.discovered-peer-menu-open = false;
                                root.select-session-tab();
                            }
                        }
                    }

                    Rectangle {
                        width: 120px;
                        background: transparent;

                        FlatButton {
                            text: "Metrics";
                            dark-mode: root.dark-mode;
                            active: root.active-tab == 1;
                            clickable: root.active-tab != 1;
                            clicked => {
                                root.bind-ip-menu-open = false;
                                root.output-device-menu-open = false;
                                root.discovered-peer-menu-open = false;
                                root.select-metrics-tab();
                            }
                        }
                    }

                    Rectangle {
                        width: 120px;
                        background: transparent;

                        FlatButton {
                            text: "Settings";
                            dark-mode: root.dark-mode;
                            active: root.active-tab == 2;
                            clickable: root.active-tab != 2;
                            clicked => {
                                root.bind-ip-menu-open = false;
                                root.output-device-menu-open = false;
                                root.discovered-peer-menu-open = false;
                                root.select-settings-tab();
                            }
                        }
                    }

                    Rectangle {
                        width: 140px;
                        background: transparent;

                        FlatButton {
                            text: "Report a Bug";
                            dark-mode: root.dark-mode;
                            clicked => {
                                root.bind-ip-menu-open = false;
                                root.output-device-menu-open = false;
                                root.discovered-peer-menu-open = false;
                                root.report-bug();
                            }
                        }
                    }

                    Rectangle { background: transparent; }
                }

                HorizontalBox {
                    spacing: 14px;

                    Rectangle {
                        width: 4px;
                        height: 88px;
                        y: (parent.height - self.height) / 2;
                        border-radius: 2px;
                        background: root.accent;
                    }

                    VerticalBox {
                        spacing: 2px;

                        Text {
                            text: "Velin";
                            color: root.text-primary;
                            font-size: 22px;
                            font-weight: 600;
                        }

                        Text {
                            text: "Local network audio transport";
                            color: root.text-secondary;
                            font-size: 12px;
                        }
                    }

                    Rectangle { background: transparent; }

                    VerticalBox {
                        spacing: 2px;

                        Text {
                            horizontal-alignment: right;
                            text: root.local-machine-name;
                            color: root.text-primary;
                            font-size: 14px;
                        }

                        Text {
                            horizontal-alignment: right;
                            text: root.bind-ip-selection == "Automatic"
                                ? "Primary IP " + root.local-primary-ip
                                : "Bind IP " + root.bind-ip-selection;
                            color: root.text-secondary;
                            font-size: 12px;
                        }
                    }
                }

                if root.active-tab == 0 : Rectangle {
                    border-color: root.border;
                    border-width: 1px;
                    border-radius: 14px;
                    background: root.panel-bg;
                    height: root.sender-mode ? 280px : 252px;

                    VerticalBox {
                        padding: 18px;
                        spacing: 12px;

                        Text {
                            text: "Session";
                            color: root.text-primary;
                            font-size: 16px;
                            font-weight: 600;
                        }

                        HorizontalBox {
                            spacing: 8px;
                            height: 34px;

                            Rectangle { background: transparent; }

                            Rectangle {
                                width: 120px;
                                background: transparent;

                                FlatButton {
                                    text: "Receiver";
                                    dark-mode: root.dark-mode;
                                    active: !root.sender-mode;
                                    clickable: !root.running;
                                    clicked => {
                                        root.sender-mode = false;
                                        root.discovered-peer-menu-open = false;
                                    }
                                }
                            }

                            Rectangle {
                                width: 120px;
                                background: transparent;

                                FlatButton {
                                    text: "Sender";
                                    dark-mode: root.dark-mode;
                                    active: root.sender-mode;
                                    clickable: !root.running;
                                    clicked => { root.sender-mode = true; }
                                }
                            }

                            Rectangle { background: transparent; }
                        }

                        Rectangle {
                            height: root.sender-mode ? 118px : 72px;
                            background: transparent;

                            if !root.sender-mode : VerticalBox {
                                y: 0px;
                                width: parent.width;
                                spacing: 8px;

                                Text {
                                    text: "Bind IP";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                Rectangle {
                                    border-color: root.border;
                                    border-width: 1px;
                                    border-radius: 9px;
                                    background: root.dark-mode ? #1c1c1c : #ffffff;
                                    height: 38px;

                                    Text {
                                        x: 12px;
                                        y: (parent.height - self.height) / 2;
                                        text: root.bind-ip-selection;
                                        color: root.text-primary;
                                        font-size: 13px;
                                    }
                                }
                            }

                            if root.sender-mode : VerticalBox {
                                y: 0px;
                                width: parent.width;
                                spacing: 8px;

                                Text {
                                    text: "Discovered receivers";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                HorizontalBox {
                                    spacing: 8px;
                                    height: 38px;

                                    Rectangle {
                                        background: transparent;

                                        Rectangle {
                                            height: 38px;
                                            border-radius: 9px;
                                            border-width: 1px;
                                            border-color: root.border;
                                            background: root.dark-mode ? #1c1c1c : #ffffff;

                                            Text {
                                                x: 12px;
                                                y: (parent.height - self.height) / 2;
                                                text: root.discovered-peer-selection;
                                                color: root.dark-mode ? #f2f2f2 : #171717;
                                                font-size: 13px;
                                            }

                                            Text {
                                                x: parent.width - self.width - 14px;
                                                y: (parent.height - self.height) / 2 - 1px;
                                                text: root.discovered-peer-menu-open ? "˄" : "˅";
                                                color: root.dark-mode ? #c9c9c9 : #5a564f;
                                                font-size: 13px;
                                            }

                                            TouchArea {
                                                enabled: !root.running && root.discovered-peer-selection != "No receivers found";
                                                clicked => {
                                                    root.discovered-peer-menu-open = !root.discovered-peer-menu-open;
                                                    root.bind-ip-menu-open = false;
                                                    root.output-device-menu-open = false;
                                                }
                                            }
                                        }
                                    }

                                    Rectangle {
                                        width: 86px;
                                        background: transparent;

                                        FlatButton {
                                            text: "Refresh";
                                            dark-mode: root.dark-mode;
                                            clickable: !root.running;
                                            clicked => {
                                                root.discovered-peer-menu-open = false;
                                                root.refresh-discovery();
                                            }
                                        }
                                    }
                                }

                                Text {
                                    text: "Receiver IP";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                FlatInput {
                                    text <=> root.target-ip;
                                    enabled: !root.running;
                                    dark-mode: root.dark-mode;
                                    placeholder: "192.168.0.x";
                                }
                            }
                        }

                        HorizontalBox {
                            spacing: 8px;

                            Rectangle { background: transparent; }

                            Rectangle {
                                width: 120px;
                                background: transparent;
                                height: 28px;

                                FlatButton {
                                    text: root.sender-mode ? "Start Sender" : "Start Receiver";
                                    dark-mode: root.dark-mode;
                                    clickable: !root.running;
                                    clicked => {
                                        if (root.sender-mode) {
                                            root.start-source(root.target-ip);
                                        } else {
                                            root.start-target();
                                        }
                                    }
                                }
                            }

                            Rectangle {
                                width: 120px;
                                background: transparent;
                                height: 28px;

                                FlatButton {
                                    text: root.muted ? "Unmute" : "Mute";
                                    dark-mode: root.dark-mode;
                                    active: root.muted;
                                    clickable: root.running;
                                    clicked => { root.toggle-mute(); }
                                }
                            }

                            Rectangle {
                                width: 120px;
                                background: transparent;
                                height: 28px;

                                FlatButton {
                                    text: "Stop";
                                    dark-mode: root.dark-mode;
                                    clickable: root.running;
                                    clicked => { root.stop-session(); }
                                }
                            }

                            Rectangle { background: transparent; }
                        }
                    }

                    if root.discovered-peer-menu-open : Rectangle {
                        x: 18px;
                        y: 152px;
                        width: parent.width - 36px;
                        height: min(root.discovered-peer-options.length * 30px + 12px, 132px);
                        border-radius: 9px;
                        border-width: 1px;
                        border-color: root.border;
                        background: root.dark-mode ? #1c1c1c : #ffffff;
                        clip: true;

                        ScrollView {
                            x: 0px;
                            y: 0px;
                            width: parent.width;
                            height: parent.height;
                            viewport-width: self.visible-width;
                            viewport-height: root.discovered-peer-options.length * 30px;

                            VerticalBox {
                                spacing: 0px;

                                for option[index] in root.discovered-peer-options : Rectangle {
                                    height: 30px;
                                    background: option == root.discovered-peer-selection
                                        ? (root.dark-mode ? #232923 : #eef3ec)
                                        : (peer-option-touch.has-hover ? (root.dark-mode ? #242424 : #f3f0ea) : transparent);

                                    Text {
                                        x: 12px;
                                        y: (parent.height - self.height) / 2;
                                        text: option;
                                        color: root.dark-mode ? #f2f2f2 : #171717;
                                        font-size: 13px;
                                    }

                                    peer-option-touch := TouchArea {
                                        clicked => {
                                            root.choose-discovered-peer(option);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if root.active-tab == 1 : Rectangle {
                    border-color: root.border;
                    border-width: 1px;
                    border-radius: 14px;
                    background: root.panel-bg;
                    height: 432px;

                    VerticalBox {
                        padding: 18px;
                        spacing: 10px;

                        Text {
                            text: "Metrics";
                            color: root.text-primary;
                            font-size: 16px;
                            font-weight: 600;
                        }

                        Text {
                            text: "Live stream health, queue depth, and reconnect state.";
                            color: root.text-secondary;
                            font-size: 12px;
                        }

                        Rectangle {
                            border-color: root.border;
                            border-width: 1px;
                            border-radius: 12px;
                            background: root.panel-alt;
                            height: 340px;

                            ScrollView {
                                x: 10px;
                                y: 10px;
                                width: parent.width - 20px;
                                height: parent.height - 20px;
                                viewport-width: self.visible-width;
                                viewport-height: max(self.visible-height, metrics-body.preferred-height + 8px);

                                metrics-body := Text {
                                    width: parent.visible-width - 8px;
                                    text: root.metrics-text;
                                    color: root.text-primary;
                                    font-size: 13px;
                                    font-family: "Cascadia Mono";
                                    wrap: word-wrap;
                                }
                            }
                        }
                    }
                }

                if root.active-tab == 2 : Rectangle {
                    border-color: root.border;
                    border-width: 1px;
                    border-radius: 14px;
                    background: root.panel-bg;
                    height: 344px;

                    VerticalBox {
                        padding: 18px;
                        spacing: 10px;

                        Text {
                            text: "Settings";
                            color: root.text-primary;
                            font-size: 16px;
                            font-weight: 600;
                        }

                        Text {
                            text: "Saved locally and used as defaults for new sessions.";
                            color: root.text-secondary;
                            font-size: 12px;
                        }

                        HorizontalBox {
                            spacing: 10px;

                            VerticalBox {
                                spacing: 6px;

                                Text {
                                    text: "Theme";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                HorizontalBox {
                                    spacing: 8px;
                                    height: 34px;

                                    FlatButton {
                                        text: "Dark";
                                        dark-mode: root.dark-mode;
                                        active: root.dark-mode;
                                        clickable: !root.running;
                                        clicked => {
                                            root.dark-mode = true;
                                            root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                        }
                                    }

                                    FlatButton {
                                        text: "Light";
                                        dark-mode: root.dark-mode;
                                        active: !root.dark-mode;
                                        clickable: !root.running;
                                        clicked => {
                                            root.dark-mode = false;
                                            root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                        }
                                    }
                                }
                            }

                            VerticalBox {
                                spacing: 6px;

                                Text {
                                    text: "Bind IP";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                Rectangle {
                                    height: 38px;
                                    background: transparent;

                                    Rectangle {
                                        height: 38px;
                                        border-radius: 9px;
                                        border-width: 1px;
                                        border-color: root.border;
                                        background: root.dark-mode ? #1c1c1c : #ffffff;

                                        Text {
                                            x: 12px;
                                            y: (parent.height - self.height) / 2;
                                            text: root.bind-ip-selection;
                                            color: root.dark-mode ? #f2f2f2 : #171717;
                                            font-size: 13px;
                                        }

                                        Text {
                                            x: parent.width - self.width - 14px;
                                            y: (parent.height - self.height) / 2 - 1px;
                                            text: root.bind-ip-menu-open ? "˄" : "˅";
                                            color: root.dark-mode ? #c9c9c9 : #5a564f;
                                            font-size: 13px;
                                        }

                                        TouchArea {
                                            enabled: !root.running;
                                            clicked => {
                                                root.bind-ip-menu-open = !root.bind-ip-menu-open;
                                                root.output-device-menu-open = false;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        VerticalBox {
                            spacing: 6px;

                            Text {
                                text: "Output device";
                                color: root.text-tertiary;
                                font-size: 12px;
                            }

                            Rectangle {
                                height: 38px;
                                background: transparent;

                                Rectangle {
                                    height: 38px;
                                    border-radius: 9px;
                                    border-width: 1px;
                                    border-color: root.border;
                                    background: root.dark-mode ? #1c1c1c : #ffffff;

                                    Text {
                                        x: 12px;
                                        y: (parent.height - self.height) / 2;
                                        text: root.output-device-selection;
                                        color: root.dark-mode ? #f2f2f2 : #171717;
                                        font-size: 13px;
                                    }

                                    Text {
                                        x: parent.width - self.width - 14px;
                                        y: (parent.height - self.height) / 2 - 1px;
                                        text: root.output-device-menu-open ? "˄" : "˅";
                                        color: root.dark-mode ? #c9c9c9 : #5a564f;
                                        font-size: 13px;
                                    }

                                    TouchArea {
                                        enabled: !root.running;
                                        clicked => {
                                            root.output-device-menu-open = !root.output-device-menu-open;
                                            root.bind-ip-menu-open = false;
                                        }
                                    }
                                }
                            }
                        }

                        HorizontalBox {
                            spacing: 10px;

                            VerticalBox {
                                spacing: 6px;

                                Text {
                                    text: "Default receiver IP";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                FlatInput {
                                    text <=> root.target-ip;
                                    enabled: !root.running;
                                    dark-mode: root.dark-mode;
                                    placeholder: "127.0.0.1";
                                    edited => {
                                        root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                    }
                                }
                            }

                            VerticalBox {
                                spacing: 6px;

                                Text {
                                    text: "Control port";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                FlatInput {
                                    text <=> root.control-port;
                                    enabled: !root.running;
                                    dark-mode: root.dark-mode;
                                    placeholder: "49000";
                                    edited => {
                                        root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                    }
                                }
                            }

                            VerticalBox {
                                spacing: 6px;

                                Text {
                                    text: "Audio port";
                                    color: root.text-tertiary;
                                    font-size: 12px;
                                }

                                FlatInput {
                                    text <=> root.audio-port;
                                    enabled: !root.running;
                                    dark-mode: root.dark-mode;
                                    placeholder: "49001";
                                    edited => {
                                        root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                    }
                                }
                            }
                        }
                    }

                    if root.bind-ip-menu-open : Rectangle {
                        x: parent.width / 2 + 5px;
                        y: 154px;
                        width: (parent.width - 46px) / 2;
                        height: root.bind-ip-options.length * 30px + 12px;
                        border-radius: 9px;
                        border-width: 1px;
                        border-color: root.border;
                        background: root.dark-mode ? #1c1c1c : #ffffff;
                        clip: true;

                        VerticalBox {
                            spacing: 0px;

                            for option[index] in root.bind-ip-options : Rectangle {
                                height: 30px;
                                background: option == root.bind-ip-selection
                                    ? (root.dark-mode ? #232923 : #eef3ec)
                                    : (bind-option-touch.has-hover ? (root.dark-mode ? #242424 : #f3f0ea) : transparent);

                                Text {
                                    x: 12px;
                                    y: (parent.height - self.height) / 2;
                                    text: option;
                                    color: root.dark-mode ? #f2f2f2 : #171717;
                                    font-size: 13px;
                                }

                                bind-option-touch := TouchArea {
                                    clicked => {
                                        root.bind-ip-selection = option;
                                        root.bind-ip-menu-open = false;
                                        root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                    }
                                }
                            }
                        }
                    }

                    if root.output-device-menu-open : Rectangle {
                        x: 18px;
                        y: 164px;
                        width: parent.width - 36px;
                        height: min(root.output-device-options.length * 30px + 12px, 162px);
                        border-radius: 9px;
                        border-width: 1px;
                        border-color: root.border;
                        background: root.dark-mode ? #1c1c1c : #ffffff;
                        clip: true;

                        ScrollView {
                            x: 0px;
                            y: 0px;
                            width: parent.width;
                            height: parent.height;
                            viewport-width: self.visible-width;
                            viewport-height: root.output-device-options.length * 30px;

                            VerticalBox {
                                spacing: 0px;

                                for option[index] in root.output-device-options : Rectangle {
                                    height: 30px;
                                    background: option == root.output-device-selection
                                        ? (root.dark-mode ? #232923 : #eef3ec)
                                        : (output-option-touch.has-hover ? (root.dark-mode ? #242424 : #f3f0ea) : transparent);

                                    Text {
                                        x: 12px;
                                        y: (parent.height - self.height) / 2;
                                        text: option;
                                        color: root.dark-mode ? #f2f2f2 : #171717;
                                        font-size: 13px;
                                    }

                                    output-option-touch := TouchArea {
                                        clicked => {
                                            root.output-device-selection = option;
                                            root.output-device-menu-open = false;
                                            root.save-settings(root.target-ip, root.bind-ip-selection, root.output-device-selection, root.control-port, root.audio-port, root.dark-mode);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if root.active-tab == 0 : Rectangle {
                    border-color: root.border;
                    border-width: 1px;
                    border-radius: 14px;
                    background: root.panel-bg;

                    VerticalBox {
                        padding: 10px;
                        spacing: 8px;

                        Text {
                            text: "Log";
                            color: root.text-primary;
                            font-size: 14px;
                            font-weight: 600;
                        }

                        Rectangle {
                            border-color: root.border;
                            border-width: 1px;
                            border-radius: 12px;
                            background: root.panel-alt;

                            ScrollView {
                                x: 10px;
                                y: 10px;
                                width: parent.width - 20px;
                                height: parent.height - 20px;
                                viewport-width: self.visible-width;
                                viewport-height: max(self.visible-height, log-body.preferred-height + 8px);

                                log-body := Text {
                                    width: parent.visible-width - 8px;
                                    text: root.log-text;
                                    color: root.text-primary;
                                    font-size: 12px;
                                    font-family: "Cascadia Mono";
                                    wrap: word-wrap;
                                }
                            }
                        }
                    }
                }

                if root.active-tab == 0 : Rectangle {
                    border-color: root.border;
                    border-width: 1px;
                    border-radius: 12px;
                    background: root.panel-alt;
                    height: 38px;

                    Rectangle {
                        x: 12px;
                        y: (parent.height - self.height) / 2;
                        width: 8px;
                        height: 8px;
                        border-radius: 4px;
                        background: root.running ? root.accent : (root.dark-mode ? #7a7a7a : #8b877f);
                    }

                    Text {
                        x: 28px;
                        y: (parent.height - self.height) / 2;
                        text: root.status-text;
                        color: root.text-primary;
                        font-size: 12px;
                    }
                }
            }
        }
    }
}
