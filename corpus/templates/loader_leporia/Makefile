compile:
	rc resource.rc
	cl -O1 -DDATA_SIZE=2047584 main.c resource.res -Fe./loader.exe
	strip loader.exe

clean:
	rm loader.exe *.obj *.res