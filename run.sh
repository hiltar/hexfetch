docker run -d --name hexfetch -m 256m -p 5555:5555 -v $(pwd)/data:/data -v $(pwd)/settings:/settings hexfetch:latest
