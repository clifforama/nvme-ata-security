/*
 * Linux userspace tool to configure ATA security on NVMe drives
 *
 * (C) Copyright 2016 Jethro G. Beekman
 *
 * This program is free software; you can redistribute it and/or modify it
 * under the terms of the GNU General Public License as published by the Free
 * Software Foundation; either version 2 of the License, or (at your option)
 * any later version.
 */

#[macro_use]
extern crate ioctl as ioctl_crate;
#[macro_use]
extern crate bitflags;
extern crate byteorder;
extern crate docopt;
extern crate rustc_serialize;
extern crate libc;
extern crate rpassword;

mod ops;
mod nvme;

use std::os::unix::io::AsRawFd;
use std::fs::File;
use std::io::{Read,Write,self};

use ops::Result;
use nvme::security::{AtaSecuritySpecific,AtaSecurityPassword};
use nvme::security::Protocol::AtaSecurity as ProtocolAtaSecurity;

macro_rules! eprintln {
    ($fmt:expr) => (let _=write!(::std::io::stderr(),concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (let _=write!(::std::io::stderr(),concat!($fmt, "\n"), $($arg)*));
}

macro_rules! eprint {
    ($fmt:expr) => (let _=write!(::std::io::stderr(),$fmt));
    ($fmt:expr, $($arg:tt)*) => (let _=write!(::std::io::stderr(),$fmt, $($arg)*));
}

fn security_protocols(f: &File, identity: &nvme::identify::IdentifyController) -> Result<Option<Vec<nvme::security::Protocol>>> {
	use byteorder::{BigEndian,ReadBytesExt};
	
	let fd=f.as_raw_fd();
	if identity.oacs().contains(nvme::identify::OACS_SECURITY) {
		let mut supported=vec![0u8;8];
		try!(ops::security_receive(fd,0,0,0,&mut supported));
		let bytes=(&supported[6..8]).read_u16::<BigEndian>().unwrap();
		if bytes>0 {
			supported.resize(bytes as usize+8,0);
			try!(ops::security_receive(fd,0,0,0,&mut supported));
			Ok(Some(supported.into_iter().skip(8).map(Into::<nvme::security::Protocol>::into).collect()))
		} else {
			Ok(Some(Vec::with_capacity(0)))
		}
	} else {
		Ok(None)
	}
}

fn ata_identify(f: &File, protocols: &[nvme::security::Protocol]) -> Result<Option<nvme::security::AtaSecurityIdentify>> {
	if !protocols.contains(&ProtocolAtaSecurity) {
		return Ok(None);
	}
	
	let mut buf=[0u8;16];
	try!(ops::security_receive(f.as_raw_fd(),ProtocolAtaSecurity.into(),0,0,&mut buf));
	Ok(Some(nvme::security::AtaSecurityIdentify::from(buf)))
}

fn query(f: &File) {
	let i=match ops::identify(f.as_raw_fd()) {
		Err(e) => {
			eprintln!("There was an error obtaining NVMe identity information:\n{:?}",e);
			return;
		},
		Ok(i) => i,
	};
	eprintln!("vid:ssvid: {:04x}:{:04x}
model: {}
serial: {}
firmware: {}
oacs: {:?}",i.vid(),i.ssvid(),std::str::from_utf8(i.sn()).unwrap(),std::str::from_utf8(i.mn()).unwrap(),std::str::from_utf8(i.fr()).unwrap(),i.oacs());
	let protocols=match security_protocols(&f,&i) {
		Err(e) => {
			eprintln!("There was an error enumerating supported NVMe security protocols:\n{:?}",e);
			return;
		},
		Ok(None) => {
			eprintln!("This drive does not support NVMe security commands.");
			return;
		},
		Ok(Some(p)) => p,
	};
	eprintln!("protocols: {:?}",protocols);
	let security=match ata_identify(&f,&protocols) {
		Err(e) => {
			eprintln!("There was an error obtaining ATA security information:\n{:?}",e);
			return;
		},
		Ok(None) => {
			eprintln!("This drive does not support ATA security commands.");
			return;
		},
		Ok(Some(s)) => s,
	};
	eprintln!("ata security: erase time: {} enhanced erase time: {}, master pwd id: {:04x} maxset: {} 
s_suprt: {} s_enabld: {} locked: {} frozen: {} pwncntex: {} en_er_sup: {}",security.security_erase_time(),security.enhanced_security_erase_time(),security.master_password_identifier(),security.maxset(),security.s_suprt(),security.s_enabld(),security.locked(),security.frozen(),security.pwncntex(),security.en_er_sup());
	if !security.s_suprt() {
		eprintln!("This drive does not support ATA security.");
	}
}

fn check_support(f: &File) -> Option<nvme::security::AtaSecurityIdentify> {
	let i=match ops::identify(f.as_raw_fd()) {
		Err(e) => {
			eprintln!("There was an error obtaining NVMe identity information:\n{:?}",e);
			return None;
		},
		Ok(i) => i,
	};
	let protocols=match security_protocols(&f,&i) {
		Err(e) => {
			eprintln!("There was an error enumerating supported NVMe security protocols:\n{:?}",e);
			return None;
		},
		Ok(None) => {
			eprintln!("This drive does not support NVMe security commands.");
			return None;
		},
		Ok(Some(p)) => p,
	};
	let security=match ata_identify(&f,&protocols) {
		Err(e) => {
			eprintln!("There was an error obtaining ATA security information:\n{:?}",e);
			return None;
		},
		Ok(None) => {
			eprintln!("This drive does not support ATA security commands.");
			return None;
		},
		Ok(Some(s)) => s,
	};
	if !security.s_suprt() {
		eprintln!("This drive does not support ATA security.");
		return None;
	}
	Some(security)
}

fn security_set_password_user(f: &File, password: [u8;32], maximum_security: bool) -> Result<()> {
	let buf: [u8;36]=AtaSecurityPassword::new(password,false,Some(maximum_security),None).into();
	ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::SetPassword as u16,0,Some(&buf))
}

fn security_set_password_master(f: &File, password: [u8;32], id: u16) -> Result<()> {
	let buf: [u8;36]=AtaSecurityPassword::new(password,true,None,Some(id)).into();
	ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::SetPassword as u16,0,Some(&buf))
}

fn security_unlock(f: &File, password: [u8;32], master: bool) -> Result<()> {
	let buf: [u8;36]=AtaSecurityPassword::new(password,master,None,None).into();
	try!(ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::Unlock as u16,0,Some(&buf)));
	ops::nvme_ioctl_reset(f.as_raw_fd())
}

fn security_erase(f: &File, password: [u8;32], master: bool, enhanced: bool) -> Result<()> {
	try!(ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::ErasePrepare as u16,0,None));
	let buf: [u8;36]=AtaSecurityPassword::new(password,master,Some(enhanced),None).into();
	ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::EraseUnit as u16,0,Some(&buf))
}

fn security_freeze(f: &File) -> Result<()> {
	ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::FreezeLock as u16,0,None)
}

fn security_disable_password(f: &File, password: [u8;32], master: bool) -> Result<()> {
	let buf: [u8;36]=AtaSecurityPassword::new(password,master,None,None).into();
	ops::security_send(f.as_raw_fd(),ProtocolAtaSecurity.into(),AtaSecuritySpecific::DisablePassword as u16,0,Some(&buf))
}

fn read_password_err(src: Option<String>, confirm: bool) -> std::result::Result<[u8;32],io::Error> {
	let mut f_file;
	let mut f_stdin;
	let f_password;
	let mut f_password_ptr;
	let f: &mut Read=if let Some(src)=src {
		f_file=try!(File::open(src));
		&mut f_file
	} else {
		if unsafe{libc::isatty(0)}==1 {
			loop {
				eprint!("Please enter password:");
				let password1=try!(rpassword::read_password());
				if password1.len()==0 {
					continue;
				} else if password1.len()>32 {
					eprintln!("Password too long!");
					continue;
				}
				if confirm {
					eprint!("Enter password again:");
					let password2=try!(rpassword::read_password());
					if password1!=password2 {
						eprintln!("Passwords don't match!");
						continue;
					}
				}
				f_password=password1;
				break;
			}
			f_password_ptr=f_password.as_bytes();
			&mut f_password_ptr
		} else {
			f_stdin=io::stdin();
			&mut f_stdin
		}
	};
	let mut buf=[0u8;32];
	io::copy(&mut f.take(32),&mut &mut buf[..]).and_then(|n|
		if n==0 {
			Err(io::Error::new(io::ErrorKind::UnexpectedEof,"zero bytes read"))
		} else {
			Ok(buf)
		})
}

fn read_password(src: Option<String>, confirm: bool) -> [u8;32] {
	match read_password_err(src,confirm) {
		Err(e) => {
			eprintln!("Error trying to read password: {}",e);
			std::process::exit(1);
		},
		Ok(v) => v,
	}
}

fn main() {
#[derive(RustcDecodable,Debug)]
#[allow(dead_code)]
struct Args {
	cmd_query: bool,
	cmd_set_password: bool,
	cmd_unlock: bool,
	cmd_disable_password: bool,
	cmd_erase: bool,
	cmd_freeze: bool,
	arg_dev: String,
	flag_password_file: Option<String>,
	flag_id: u16,
	flag_user: bool,
	flag_master: bool,
	flag_high: bool,
	flag_max: bool,
    flag_enhanced: bool,
}

const USAGE: &'static str = "
Usage:
	nvme-ata-security query <dev>
	nvme-ata-security set-password -u (--high|--max) [--password-file=<file>] <dev>
	nvme-ata-security set-password -m --id=<id> [--password-file=<file>] <dev>
	nvme-ata-security unlock (-u|-m) [--password-file=<file>] <dev>
	nvme-ata-security disable-password (-u|-m) [--password-file=<file>] <dev>
	nvme-ata-security erase (-u|-m) [--enhanced] [--password-file=<file>] <dev>
	nvme-ata-security freeze <dev>
	nvme-ata-security --help
	
Options:
    -u, --user                         Specify the user password
    -m, --master                       Specify the master password
    -i <file>, --password-file=<file>  Read the password from <file> instead of stdin
    --high                             Configure high security
    --max                              Configure maximum security
    --id=<id>                          Set the master password identifier
    --enhanced                         Perform an enhanced security erase
";

	let args: Args = docopt::Docopt::new(USAGE).and_then(|d|d.argv(std::env::args()).decode()).unwrap_or_else(|e|e.exit());
	let f=match File::open(&args.arg_dev) {
		Err(e) => {
			eprintln!("Unable to open {} for reading: {}",args.arg_dev,e);
			return;
		},
		Ok(f) => f,
	};
	
	if args.cmd_query {
		query(&f);
		return;
	} else {
		check_support(&f);
	}
	
	let result=if args.cmd_set_password {
		eprintln!("Performing SECURITY SET PASSWORD...");
		if args.flag_user {
			security_set_password_user(&f,read_password(args.flag_password_file,true),args.flag_max)
		} else {
			security_set_password_master(&f,read_password(args.flag_password_file,true),args.flag_id)
		}
	} else if args.cmd_unlock {
		eprintln!("Performing SECURITY UNLOCK...");
		security_unlock(&f,read_password(args.flag_password_file,false),args.flag_master)
	} else if args.cmd_disable_password {
		eprintln!("Performing SECURITY DISABLE PASSWORD...");
		security_disable_password(&f,read_password(args.flag_password_file,false),args.flag_master)
	} else if args.cmd_erase {
		eprintln!("Performing SECURITY ERASE...");
		security_erase(&f,read_password(args.flag_password_file,true),args.flag_master,args.flag_enhanced)
	} else if args.cmd_freeze {
		eprintln!("Performing SECURITY FREEZE...");
		security_freeze(&f)
	} else {
		unreachable!()
	};
	
	if let Err(e)=result {
		eprintln!("There was an error executing the command: {:?}",e);
	} else {
		eprintln!("Success!");
	}
}
