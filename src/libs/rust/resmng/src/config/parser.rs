/*
 * Copyright (C) 2020-2022 Nils Asmussen, Barkhausen Institut
 *
 * This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 2 as
 * published by the Free Software Foundation.
 *
 * M3 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 2 for more details.
 */

use m3::col::{String, ToString, Vec};
use m3::format;
use m3::kif;
use m3::rc::Rc;
use m3::tcu::{Label, UNLIM_CREDITS};
use m3::util::parse;

use crate::config;

struct ConfigParser {
    chars: Vec<char>,
    pos: usize,
}

impl ConfigParser {
    fn new(xml: &str) -> Self {
        ConfigParser {
            chars: xml.chars().collect(),
            pos: 0,
        }
    }

    fn get(&mut self) -> Option<char> {
        if self.pos < self.chars.len() {
            let idx = self.pos;
            self.pos += 1;
            Some(self.chars[idx])
        }
        else {
            None
        }
    }

    fn put(&mut self) -> Option<char> {
        if self.pos > 0 {
            self.pos -= 1;
            Some(self.chars[self.pos])
        }
        else {
            None
        }
    }

    fn finish(&mut self) -> Option<()> {
        while self.pos < self.chars.len() {
            if !self.chars[self.pos].is_whitespace() {
                return None;
            }
            self.pos += 1;
        }
        Some(())
    }

    fn get_no_ws(&mut self) -> Option<char> {
        loop {
            let c = self.get()?;
            if c.is_whitespace() {
                continue;
            }
            break Some(c);
        }
    }

    fn consume(&mut self, c: char) -> Option<()> {
        let nc = self.get_no_ws()?;
        if nc != c {
            None
        }
        else {
            Some(())
        }
    }

    fn parse_ident(&mut self, delim: char) -> Option<String> {
        let mut name_buf = String::new();
        let first = self.get_no_ws()?;
        name_buf.push(first);

        while let Some(c) = self.get() {
            if c == delim {
                self.put();
                break;
            }
            if c.is_whitespace() {
                break;
            }

            name_buf.push(c);
        }
        Some(name_buf)
    }

    fn parse_arg(&mut self) -> Option<Option<(String, String)>> {
        let first = self.get_no_ws()?;
        self.put();
        if first == '>' || first == '/' {
            return Some(None);
        }

        let name = self.parse_ident('=')?;
        self.consume('=')?;
        self.consume('"')?;

        let mut val_buf = String::new();
        while let Some(c) = self.get() {
            if c == '"' {
                break;
            }

            val_buf.push(c);
        }
        Some(Some((name, val_buf)))
    }

    fn parse_tag_name(&mut self) -> Option<Option<String>> {
        self.consume('<')?;

        let mut name_buf = String::new();
        let first = self.get_no_ws()?;

        if first == '/' {
            while let Some(n) = self.put() {
                if n == '<' {
                    return Some(None);
                }
            }
        }
        name_buf.push(first);

        while let Some(c) = self.get() {
            if c.is_whitespace() {
                break;
            }
            if c == '>' || c == '/' {
                self.put();
                break;
            }

            name_buf.push(c);
        }

        Some(Some(name_buf))
    }
}

pub(crate) fn parse(xml: &str) -> Option<config::AppConfig> {
    let mut p = ConfigParser::new(xml);

    let app = match p.parse_tag_name()? {
        Some(tag) if tag == "app" => parse_app(&mut p, 0),
        _ => None,
    }?;

    p.finish()?;
    Some(app)
}

fn parse_app(p: &mut ConfigParser, start: usize) -> Option<config::AppConfig> {
    let mut app = config::AppConfig::default();

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "args" => {
                    for (i, a) in v.split_whitespace().enumerate() {
                        if i == 0 {
                            app.name = a.to_string();
                        }
                        app.args.push(a.to_string());
                    }
                },
                "usermem" => app.user_mem = Some(parse::size(&v)?),
                "kernmem" => app.kern_mem = Some(parse::size(&v)?),
                "time" => app.time = Some(parse::time(&v)?),
                "pagetables" => app.pts = Some(parse::int(&v)? as usize),
                "eps" => app.eps = Some(parse::int(&v)? as usize),
                "daemon" => app.daemon = parse::bool(&v)?,
                "attestation_id" => app.attestation_id = parse::int(&v)? as u32,
                "getinfo" => app.getinfo = parse::bool(&v)?,
                _ => return None,
            },
        }
    }

    if app.name.is_empty() || app.args.is_empty() {
        return None;
    }

    // put all apps that belong to the same domain as `app` into a pseudo domain
    let mut pseudo_dom = config::Domain {
        pseudo: true,
        tile: config::TileType("core".to_string()),
        ..Default::default()
    };

    let nc = p.get_no_ws()?;
    if nc == '/' {
        p.consume('>')?;
    }
    else if nc == '>' {
        let mut app_start = p.pos;
        while let Some(tag) = p.parse_tag_name()? {
            match tag.as_ref() {
                "app" => pseudo_dom.apps.push(Rc::new(parse_app(p, app_start)?)),
                "dom" => app.domains.push(parse_domain(p)?),
                "mount" => app.mounts.push(parse_mount(p)?),
                "sess" => app.sessions.push(parse_session(p)?),
                "sesscrt" => app.sesscrt.push(parse_sesscrt(p)?),
                "serv" => app.services.push(parse_service(p)?),
                "mod" => app.mods.push(parse_mod(p)?),
                "tiles" => app.tiles.push(parse_tile(p)?),
                "rgate" => app.rgates.push(parse_rgate(p)?),
                "sgate" => app.sgates.push(parse_sgate(p)?),
                "sem" => app.sems.push(parse_sem(p)?),
                "shmem" => app.shmems.push(parse_shmem(p)?),
                "serial" => app.serial = Some(config::SerialDesc::default()),
                _ => return None,
            }

            if tag != "dom" && tag != "app" {
                p.consume('/')?;
                p.consume('>')?;
            }
            app_start = p.pos;
        }
        parse_close_tag(p, "app")?;
    }
    else {
        return None;
    }

    if !pseudo_dom.apps.is_empty() {
        app.domains.insert(0, pseudo_dom);
    }

    app.cfg_range = (start, p.pos);
    // don't collect session creators for root
    if start != 0 {
        let mut crts = Vec::new();
        collect_sess_crts(&app, &mut crts);

        for c in crts {
            let duplicate = app.sesscrt.iter().any(|sc| sc.serv_name() == c.serv_name());
            if !duplicate && !hosts_service(&app, c.serv_name()) {
                app.sesscrt.push(c);
            }
        }
    }

    Some(app)
}

fn hosts_service(app: &config::AppConfig, name: &str) -> bool {
    for d in app.domains() {
        for a in d.apps() {
            if hosts_service(a, name) || a.services().iter().any(|s| s.name().global() == name) {
                return true;
            }
        }
    }
    false
}

fn collect_sess_crts(app: &config::AppConfig, crts: &mut Vec<config::SessCrtDesc>) {
    for d in app.domains() {
        for a in d.apps() {
            for s in a.sessions() {
                if s.is_dep() {
                    crts.push(config::SessCrtDesc::new(s.name().global().clone(), None));
                }
            }
            collect_sess_crts(a, crts);
        }
    }
}

fn parse_dual_name(dual: &mut config::DualName, n: String, v: String) -> Option<()> {
    match n.as_ref() {
        "name" => {
            dual.local.clone_from(&v);
            dual.global = v
        },
        "lname" => dual.local = v,
        "gname" => dual.global = v,
        _ => return None,
    }
    Some(())
}

fn parse_domain(p: &mut ConfigParser) -> Option<config::Domain> {
    let mut dom = config::Domain::default();

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "tile" => dom.tile = config::TileType(v),
                "mux" => dom.mux = Some(v),
                "muxmem" => dom.mux_mem = Some(parse::size(&v)?),
                "initrd" => dom.initrd = Some(v),
                "dtb" => dom.dtb = Some(v),
                "tee" => dom.tee = parse::int(&v)? == 1,
                _ => return None,
            },
        }
    }

    if dom.tile.0.is_empty() {
        dom.tile = config::TileType("core".to_string());
    }

    p.consume('>')?;

    let mut app_start = p.pos;
    while let Some(tag) = p.parse_tag_name()? {
        match tag.as_str() {
            "app" => {
                dom.apps.push(Rc::new(parse_app(p, app_start)?));
            },
            "shmem" => {
                dom.shmems.push(parse_shmem(p)?);
                p.consume('/')?;
                p.consume('>')?;
            },
            _ => return None,
        }

        app_start = p.pos;
    }

    parse_close_tag(p, "dom")?;
    Some(dom)
}

fn parse_shmem(p: &mut ConfigParser) -> Option<config::ShMemDesc> {
    let mut name = String::new();

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "name" => name.clone_from(&v),
                _ => return None,
            },
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::ShMemDesc::new(name))
    }
}

fn parse_mount(p: &mut ConfigParser) -> Option<config::MountDesc> {
    let mut fs = String::new();
    let mut path = String::new();

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "fs" => fs.clone_from(&v),
                "path" => {
                    if v.ends_with('/') {
                        path.clone_from(&v);
                    }
                    else {
                        path = format!("{}/", v);
                    }
                },
                _ => return None,
            },
        }
    }

    if fs.is_empty() || path.is_empty() {
        None
    }
    else {
        Some(config::MountDesc::new(fs, path))
    }
}

fn parse_mod(p: &mut ConfigParser) -> Option<config::ModDesc> {
    let mut name = config::DualName::default();
    let mut perm = kif::Perm::RWX;

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "name" | "lname" | "gname" => parse_dual_name(&mut name, n, v)?,
                "perm" => perm = parse::perm(&v)?,
                _ => return None,
            },
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::ModDesc::new(name, perm))
    }
}

fn parse_service(p: &mut ConfigParser) -> Option<config::ServiceDesc> {
    let mut name = config::DualName::default();

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => parse_dual_name(&mut name, n, v)?,
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::ServiceDesc::new(name))
    }
}

fn parse_sesscrt(p: &mut ConfigParser) -> Option<config::SessCrtDesc> {
    let mut name = String::new();
    let mut count = None;

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "name" => name = v,
                "count" => count = Some(parse::int(&v)? as u32),
                _ => return None,
            },
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::SessCrtDesc::new(name, count))
    }
}

fn parse_session(p: &mut ConfigParser) -> Option<config::SessionDesc> {
    let mut name = config::DualName::default();
    let mut arg = String::new();
    let mut dep = true;

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "name" | "lname" | "gname" => parse_dual_name(&mut name, n, v)?,
                "args" => arg = v,
                "dep" => dep = parse::bool(&v)?,
                _ => return None,
            },
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::SessionDesc::new(name, arg, dep))
    }
}

fn parse_tile(p: &mut ConfigParser) -> Option<config::TileDesc> {
    let mut ty = String::new();
    let mut count = 1;
    let mut mux = Some(String::from("tilemux"));
    let mut optional = false;

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "type" => ty = v,
                "count" => count = parse::int(&v)? as u32,
                "mux" => mux = if v == "-" { None } else { Some(v) },
                "optional" => optional = parse::bool(&v)?,
                _ => return None,
            },
        }
    }

    if ty.is_empty() {
        None
    }
    else {
        Some(config::TileDesc::new(ty, count, mux, optional))
    }
}

fn parse_rgate(p: &mut ConfigParser) -> Option<config::RGateDesc> {
    let mut name = config::DualName::default();
    let mut msg_size = 64;
    let mut slots = 1;

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "name" | "lname" | "gname" => parse_dual_name(&mut name, n, v)?,
                "msgsize" => msg_size = parse::int(&v)? as usize,
                "slots" => slots = parse::int(&v)? as usize,
                _ => return None,
            },
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::RGateDesc::new(name, msg_size, slots))
    }
}

fn parse_sgate(p: &mut ConfigParser) -> Option<config::SGateDesc> {
    let mut name = config::DualName::default();
    let mut credits = 1;
    let mut label = 0;

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => match n.as_ref() {
                "name" | "lname" | "gname" => parse_dual_name(&mut name, n, v)?,
                "credits" => credits = parse::int(&v)? as u32,
                "label" => label = parse::int(&v)? as Label,
                _ => return None,
            },
        }
    }

    if name.is_empty() {
        None
    }
    else {
        if credits == 0 {
            credits = UNLIM_CREDITS;
        }
        Some(config::SGateDesc::new(name, credits, label))
    }
}

fn parse_sem(p: &mut ConfigParser) -> Option<config::SemDesc> {
    let mut name = config::DualName::default();

    loop {
        match p.parse_arg()? {
            None => break,
            Some((n, v)) => parse_dual_name(&mut name, n, v)?,
        }
    }

    if name.is_empty() {
        None
    }
    else {
        Some(config::SemDesc::new(name))
    }
}

fn parse_close_tag(p: &mut ConfigParser, name: &str) -> Option<()> {
    p.consume('<')?;
    p.consume('/')?;

    let tname = p.parse_ident('>')?;
    if tname != name {
        None
    }
    else {
        p.consume('>')
    }
}
