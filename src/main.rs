use core::{fmt, str};
use std::collections::HashMap;
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Sender, Receiver, channel};
use std::{result, thread};
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

type Result<T> = result::Result<T, ()>;

const SAFE_MODE: bool = true; //true for hiding sensitive information such as hosting IP
const BAN_LIMIT: Duration = Duration::from_secs(10*60);
const MESSAGE_RATE: Duration = Duration::from_secs(1);
const STRIKE_LIMIT: i32 = 10;
struct Sensitive<T> (T);


impl<T: fmt::Display> fmt::Display for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(inner) = self;
        if SAFE_MODE {
            writeln!(f, "[REDACTED]")
        }else{
            inner.fmt(f)
        }
    }
}

enum Message{
    ClientConnected{
        author: Arc<TcpStream>
    },
    ClientDisconnected{
        author_addr: SocketAddr
    },
    NewMessage{
        author_addr: SocketAddr,
        bytes: Vec<u8>
    },
}

struct Client {
    conn: Arc<TcpStream>,
    last_message: SystemTime,
    strike_count: i32
}

fn server(messages: Receiver<Message>) -> Result<()> {
    let mut clients = HashMap::new();
    let mut banned_mfs = HashMap::<IpAddr, SystemTime>::new();
    loop{
        let msg = messages.recv().expect("The Server Receiver is not hung up"); //see the main method for explaination
        match msg{
            Message::ClientConnected{author} => {

                let author_addr = author.peer_addr().expect("TODO: cache the peer address of the connection");
                let mut banned_at = banned_mfs.remove(&author_addr.ip());
                let now = SystemTime::now();

                banned_at = banned_at.and_then(|banned_at| {
                    let diff = now.duration_since(banned_at).expect("TODO: dont crash if the clock went backwards");
                    if diff >= BAN_LIMIT {
                        None
                    } else {
                        Some(banned_at)
                    }
                });

                if let Some(banned_at) = banned_at {
                    let diff = now.duration_since(banned_at).expect("TODO: dont crash if the clock went backwards");
                    banned_mfs.insert(author_addr.ip().clone(), banned_at);  
                    let mut author = author.as_ref();
                    let secs = (BAN_LIMIT-diff).as_secs_f32();
                    println!("INFO: CLient {author_addr} tried to connect, but that MF is banned for {secs} secs");
                    let _ = writeln!(author, "your are banned MF: {secs} secs left").map_err(|err| {
                        eprintln!("Could not send banned message to {author_addr}: {err}");
                    });

                   let _=  author.shutdown(Shutdown::Both).map_err(|err| {
                    eprintln!("Could not shutdown socket for {author_addr}: {err}");
                   });
                } 
                else {
                    println!("INFO: Client {author_addr} connected");
                    clients.insert(author_addr.clone(), Client { 
                        conn: author.clone(),
                        last_message: now,
                        strike_count: 0
                    });    
                }
            },
            Message::ClientDisconnected{author_addr} => {
                println!("INFO: client {author_addr} disconnected");
                clients.remove(&author_addr);
            },
            Message::NewMessage{author_addr, bytes} => {
                if let Some(author) = clients.get_mut(&author_addr) {
                    let now = SystemTime::now();
                    let diff = now.duration_since(author.last_message).expect("TODO: dont crash if the clock went backwards");
                    if diff >= MESSAGE_RATE {
                        if let Ok(_text) = str::from_utf8(&bytes) {
                            println!("INFO: client {author_addr} sent message {bytes:?}"); // here the bytes is a vec type, :? is used to print complex data structures
                            for (addr, client) in clients.iter() {
                                if *addr != author_addr  {
                                    let _ = client.conn.as_ref().write(&bytes).map_err(|err| {
                                        eprintln!("ERROR: could not broadcast message to all the clients from {author_addr}: {err}");
                                    });                               
                                }
                            }
                        } else{
                            author.strike_count  += 1;
                            if author.strike_count >= STRIKE_LIMIT {
                                println!("INFO: {author_addr} got banned");
                                banned_mfs.insert(author_addr.ip().clone(), now);
                                let _ = writeln!(author.conn.as_ref(), "You are banned MF").map_err(|err|{
                                    eprintln!("ERROR: could not write to message to {author_addr}: {err}");
                                });
                                let _ = author.conn.shutdown(Shutdown::Both).map_err(|err| {
                                    eprintln!("ERROR: could not shutdown socket for {author_addr}: {err}");
                                });
                            }
                        }
                    } else {
                        author.strike_count  += 1;
                        if author.strike_count >= STRIKE_LIMIT {
                            println!("INFO: {author_addr} got banned");
                            banned_mfs.insert(author_addr.ip().clone(), now);
                            let _ = writeln!(author.conn.as_ref(), "You are banned MF").map_err(|err|{
                                eprintln!("ERROR: could not write to message to {author_addr}: {err}");
                            });
                            let _ = author.conn.shutdown(Shutdown::Both).map_err(|err| {
                                eprintln!("ERROR: could not shutdown socket for {author_addr}: {err}");
                            });
                        }
                    }
                }
            }
        }
    }
}

fn client (stream: Arc<TcpStream>, messages: Sender<Message>) -> Result<()> {
    let author_addr = stream.as_ref().peer_addr().map_err(|err| {
    eprintln!("Error getting peer address: {err}");
    })?;
    messages.send(Message::ClientConnected{author:stream.clone()}).map_err(|err| {
        eprintln!("ERROR: could not send message to the server thread: {err}")
    })?;
    let mut buffer = Vec::new();
    buffer.resize(64, 0);
    
    loop{
        //.as_ref() is used to pass the stream as a reference
        let n = stream.as_ref().read(&mut buffer).map_err(|err| { 
            eprintln!("ERROR: could not read message {author_addr}: {err}",author_addr = Sensitive(author_addr), err = Sensitive(err)); 
            let _= messages.send(Message::ClientDisconnected{author_addr}).map_err(|err|{
                eprintln!("ERROR: could not send message to the server thread: {err}")
            });
        })?;
        if n>0 {
            let mut bytes=  Vec::new();
            for x in &buffer[0..n] {
                if *x >= 32 {
                    bytes.push(*x)
                }
            }
            messages.send(Message::NewMessage{author_addr, bytes}).map_err(|err| {
            eprintln!("ERROR: could not send message to the server thread: {err}");
            buffer[0..n].to_vec();
        })?;
    } else{
        let _ = messages.send(Message::ClientDisconnected { author_addr }).map_err(|err|{
            eprintln!("ERROR: could not send message to the server thread: {err} ");
        });
        break;
        }
    }
    Ok(())

}

fn main() -> Result<()> {
    let address = "0.0.0.0:6969";
    let listener = TcpListener::bind(address).map_err(|err| {
        eprintln!("ERROR: could not bind {address}: {err}", err=Sensitive(err));
    })?;
    println!("INFO: Listening to the {}", Sensitive(address));
    
    let (message_sender, message_receiver) = channel();
    thread::spawn(|| server(message_receiver));

    for stream in listener.incoming(){
        match stream {
            Ok(stream) => {
                let stream = Arc::new(stream);
                let message_sender = message_sender.clone(); 
                //by cloning it we will always have the original instance available and therefore we used expect in the server msg.recv()
                thread::spawn(|| client(stream, message_sender));  
                //we can use the .into() function converts the provided type into the required type,
                //so instead of shadowing the stream with Arc::new() we can simply pass stream.into() while calling the client function and it will work fine 
            }
            Err(err) => {
                eprintln!("ERR: couldn't accept connection: {err}");
            }
        }
    }
Ok(())
}