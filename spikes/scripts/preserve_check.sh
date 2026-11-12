#!/bin/zsh
# usage: preserve_check.sh <workdir> <dest-dir>   -- verifies what each copy method preserves.
W=$1; D=$2; B=$(dirname $0)/../target/release/s3_copy
mkdir -p $W/pres $D/pres; S=$W/pres/src.dat; SP=$W/pres/sparse.dat
chmod -N $S 2>/dev/null; rm -f $S $SP; head -c 100000 /dev/urandom > $S
xattr -w com.example.custom "hello-xattr" $S
chmod +a "everyone deny delete" $S
python3 -c "import plistlib,sys;sys.stdout.buffer.write(plistlib.dumps(['Red\n6','Work'],fmt=plistlib.FMT_BINARY))" | xxd -p | tr -d '\n' > $W/pres/tags.hex
xattr -wx com.apple.metadata:_kMDItemUserTags "$(cat $W/pres/tags.hex)" $S
python3 -c "import plistlib,sys;sys.stdout.buffer.write(plistlib.dumps('a finder comment',fmt=plistlib.FMT_BINARY))" | xxd -p | tr -d '\n' > $W/pres/cmt.hex
xattr -wx com.apple.metadata:kMDItemFinderComment "$(cat $W/pres/cmt.hex)" $S
head -c 5000 /dev/urandom > $S/..namedfork/rsrc 2>/dev/null
# sparse: 1 GiB logical, 2 x 4 KiB data
python3 - <<PY
f=open("$SP","wb"); f.write(b"A"*4096); f.seek(512*1024*1024); f.write(b"B"*4096); f.truncate(1<<30); f.close()
PY
fp() { # prints a normalised fingerprint of file $1 (data fork) 
  local f=$1
  echo "xattrs=$(xattr $f | sort | tr '\n' ',')"
  echo "xattr_custom=$(xattr -p com.example.custom $f 2>/dev/null)"
  echo "tags=$(xattr -px com.apple.metadata:_kMDItemUserTags $f 2>/dev/null | tr -d ' \n' | md5)"
  echo "comment=$(xattr -px com.apple.metadata:kMDItemFinderComment $f 2>/dev/null | tr -d ' \n' | md5)"
  echo "acl=$(ls -lde $f | tail -n +2 | sed 's/^ *//' | tr '\n' ';')"
  echo "rsrc_bytes=$(stat -f %z $f/..namedfork/rsrc 2>/dev/null)"
  echo "mode_mtime=$(stat -f '%Sp %m' $f)"
  echo "btime=$(stat -f %B $f)"
  echo "mdls_tags=$(mdls -raw -name kMDItemUserTags $f 2>/dev/null | tr '\n' ' ')"
}
fp $S > $W/pres/src.fp
printf "%-26s %-7s %-7s %-7s %-7s %-7s %-7s %-7s %-7s | sparse: size / KiB on disk\n" method xattr tags comment acl rsrc mtime/mode btime mdls
sp_src="$(stat -f %z $SP) / $(du -k $SP | cut -f1)"
echo "(source sparse: $sp_src; source data file fingerprint saved)"
for m in copyfile_all copyfile_all_clone clonefile rw:1024:n rw:1024:n:1:1 rw:1024:n:1:1:0 cp cp-p cp-c ditto ditto-rsrc copyfile_all_sparse; do
  for tgtdir in $W/pres/out $D/pres; do
    mkdir -p $tgtdir; chmod -N $tgtdir/o.dat 2>/dev/null; rm -f $tgtdir/o.dat $tgtdir/o.sp
    $B one $m $S $tgtdir/o.dat 2>/dev/null; rc=$?
    $B one $m $SP $tgtdir/o.sp 2>/dev/null
    lbl="$m@$( [ $tgtdir = $D/pres ] && echo other || echo same )"
    if [ ! -f $tgtdir/o.dat ]; then printf "%-34s FAILED\n" $lbl; continue; fi
    fp $tgtdir/o.dat > $W/pres/dst.fp
    f() { # compare one field
      local a=$(grep "^$1" $W/pres/src.fp); local b=$(grep "^$1" $W/pres/dst.fp); [ "$a" = "$b" ] && echo yes || echo NO
    }
    printf "%-34s %-7s %-7s %-7s %-7s %-7s %-7s %-7s %-7s | %s / %s\n" $lbl $(f xattrs) $(f tags) $(f comment) $(f acl) $(f rsrc_bytes) $(f mode_mtime) $(f btime) $(f mdls_tags) "$(stat -f %z $tgtdir/o.sp 2>/dev/null)" "$(du -k $tgtdir/o.sp 2>/dev/null| cut -f1)"
  done
done
